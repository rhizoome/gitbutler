use std::path::PathBuf;

use anyhow::{Context, Result};
use git2::Commit;
use gitbutler_branch::BranchExt;
use gitbutler_commit::commit_headers::CommitHeadersV2;
use gitbutler_oplog::SnapshotExt;
use gitbutler_oxidize::{git2_to_gix_object_id, GixRepositoryExt, OidExt, RepoExt};
use gitbutler_oxidize::{gix_to_git2_oid, ObjectIdExt};
use gitbutler_project::access::WorktreeWritePermission;
use gitbutler_reference::{normalize_branch_name, ReferenceName, Refname};
use gitbutler_repo::RepositoryExt;
use gitbutler_repo::SignaturePurpose;
use gitbutler_repo_actions::RepoActionsExt;
use gitbutler_stack::{Stack, StackId};
use gitbutler_workspace::workspace_base;
use tracing::instrument;

use super::BranchManager;
use crate::r#virtual as vbranch;
use crate::{get_applied_status, hunk::VirtualBranchHunk, VirtualBranchesExt};

impl BranchManager<'_> {
    // to unapply a branch, we need to write the current tree out, then remove those file changes from the wd
    #[instrument(level = tracing::Level::DEBUG, skip(self, perm), err(Debug))]
    pub fn save_and_unapply(
        &self,
        stack_id: StackId,
        perm: &mut WorktreeWritePermission,
    ) -> Result<ReferenceName> {
        let vb_state = self.ctx.project().virtual_branches();
        let target_commit = self
            .ctx
            .repo()
            .find_commit(vb_state.get_default_target()?.sha)?;

        let mut target_stack = vb_state.get_stack(stack_id)?;

        // Convert the vbranch to a real branch
        let real_branch = self.build_real_branch(&mut target_stack)?;

        self.unapply(stack_id, perm, &target_commit, false)?;

        vb_state.update_ordering()?;

        // Ensure we still have a default target
        vbranch::ensure_selected_for_changes(&vb_state)
            .context("failed to ensure selected for changes")?;

        crate::integration::update_workspace_commit(&vb_state, self.ctx)?;

        real_branch.reference_name()
    }

    #[instrument(level = tracing::Level::DEBUG, skip(self, perm), err(Debug))]
    pub(crate) fn unapply(
        &self,
        stack_id: StackId,
        perm: &mut WorktreeWritePermission,
        target_commit: &Commit,
        delete_vb_state: bool,
    ) -> Result<()> {
        let vb_state = self.ctx.project().virtual_branches();
        let Some(stack) = vb_state.try_stack(stack_id)? else {
            return Ok(());
        };

        // We don't want to try unapplying branches which are marked as not in workspace by the new metric
        if !stack.in_workspace {
            return Ok(());
        }

        _ = self
            .ctx
            .project()
            .snapshot_branch_deletion(stack.name.clone(), perm);

        let repo = self.ctx.repo();

        let base_tree_id = target_commit
            .tree()
            .context("failed to get target tree")?
            .id();

        let applied_statuses = get_applied_status(self.ctx, None)
            .context("failed to get status by branch")?
            .branches;

        // doing this earlier in the flow, in case any of the steps that follow fail
        vb_state
            .mark_as_not_in_workspace(stack.id)
            .context("Failed to remove branch")?;

        if self.ctx.app_settings().feature_flags.v3 {
            // On v3 we want to take the `current_wd_tree` and try to extract
            // whatever branch we want to unapply. There are a handful of ways
            // to achieve this, including calculating the inverse diff and
            // applying that.
            //
            // We can however do more or less what `git revert` does, and
            // perform a three way merge where the `ours` side is the cwdt, the
            // `theirs` side is the workspace root, and the `base` is the head
            // of the branch we want to unapply.
            //
            // In order to handle locked files, I'm going to choose to
            // resolve conflicts in the favor of `ours` (the cwdt) which will
            // keep any locked changes in the cwdt.

            let gix_repo = self.ctx.gix_repo()?;
            let merge_options = gix_repo
                .tree_merge_options()?
                .with_file_favor(Some(gix::merge::tree::FileFavor::Ours))
                .with_tree_favor(Some(gix::merge::tree::TreeFavor::Ours));

            let cwdt = repo.create_wd_tree(0)?.id().to_gix();
            let workspace_base = gix_repo
                .find_commit(workspace_base(self.ctx, perm.read_permission())?)?
                .tree_id()?;
            let stack_head = gix_repo
                .find_commit(stack.head(&gix_repo)?.to_gix())?
                .tree_id()?;

            let mut merge = gix_repo.merge_trees(
                stack_head,
                cwdt,
                workspace_base,
                gix_repo.default_merge_labels(),
                merge_options,
            )?;
            let tree = merge.tree.write()?;
            let tree = repo.find_tree(tree.to_git2())?;

            repo.checkout_tree_builder(&tree)
                .force()
                .checkout()
                .context("failed to checkout tree")?;
        } else {
            // On v2 we can pretty just gather up the `branch.tree`s of the
            // remaining branches and check them out.

            // go through the other applied branches and merge them into the final tree
            // then check that out into the working directory
            let final_tree = {
                let _span = tracing::debug_span!(
                    "new tree without deleted branch",
                    num_branches = applied_statuses.len() - 1
                )
                .entered();
                let gix_repo = self.ctx.gix_repo()?;
                let merge_options = gix_repo.tree_merge_options()?;
                let final_tree_id = applied_statuses
                    .into_iter()
                    .filter(|(stack, _)| stack.id != stack_id)
                    .try_fold(
                        git2_to_gix_object_id(target_commit.tree_id()),
                        |final_tree_id, status| -> Result<_> {
                            let stack = status.0;
                            let files = status
                                .1
                                .into_iter()
                                .map(|file| (file.path, file.hunks))
                                .collect::<Vec<(PathBuf, Vec<VirtualBranchHunk>)>>();
                            let tree_oid = gitbutler_diff::write::hunks_onto_oid(
                                self.ctx,
                                stack.head(&gix_repo)?,
                                files,
                            )?;
                            let mut merge = gix_repo.merge_trees(
                                git2_to_gix_object_id(base_tree_id),
                                final_tree_id,
                                git2_to_gix_object_id(tree_oid),
                                gix_repo.default_merge_labels(),
                                merge_options.clone(),
                            )?;
                            let final_tree_id = merge.tree.write()?.detach();
                            Ok(final_tree_id)
                        },
                    )?;
                repo.find_tree(gix_to_git2_oid(final_tree_id))?
            };

            let _span = tracing::debug_span!("checkout final tree").entered();
            // checkout final_tree into the working directory
            repo.checkout_tree_builder(&final_tree)
                .force()
                .remove_untracked()
                .checkout()
                .context("failed to checkout tree")?;
        }

        if delete_vb_state {
            self.ctx.delete_branch_reference(&stack)?;
        }

        vbranch::ensure_selected_for_changes(&vb_state)
            .context("failed to ensure selected for changes")?;

        crate::integration::update_workspace_commit(&vb_state, self.ctx)
            .context("failed to update gitbutler workspace")?;

        Ok(())
    }
}

impl BranchManager<'_> {
    #[instrument(level = tracing::Level::DEBUG, skip(self, stack), err(Debug))]
    fn build_real_branch(&self, stack: &mut Stack) -> Result<git2::Branch<'_>> {
        let repo = self.ctx.repo();
        let target_commit = repo.find_commit(stack.head(&repo.to_gix()?)?)?;
        let branch_name = normalize_branch_name(&stack.id.to_string())?;

        let vb_state = self.ctx.project().virtual_branches();
        let branch = repo.branch(&branch_name, &target_commit, true)?;
        stack.source_refname = Some(Refname::try_from(&branch)?);
        vb_state.set_stack(stack.clone())?;

        // We don't want to write out WIP commits like this in v3 (or
        // potentially at all in v3)
        if !self.ctx.app_settings().feature_flags.v3 {
            self.build_wip_commit(stack, &branch)?;
        }

        Ok(branch)
    }

    fn build_wip_commit(
        &self,
        stack: &mut Stack,
        branch: &git2::Branch<'_>,
    ) -> Result<Option<git2::Oid>> {
        let repo = self.ctx.repo();

        // Build wip tree as either any uncommitted changes or an empty tree
        let vbranch_wip_tree = repo.find_tree(stack.tree)?;
        let vbranch_head_tree = repo.find_commit(stack.head(&repo.to_gix()?)?)?.tree()?;

        let tree = if vbranch_head_tree.id() != vbranch_wip_tree.id() {
            vbranch_wip_tree
        } else {
            // Don't create the wip commit if not required
            return Ok(None);
        };

        // Build commit message
        let mut message = "GitButler WIP Commit".to_string();
        message.push_str("\n\n");

        // Commit wip commit
        let committer = gitbutler_repo::signature(SignaturePurpose::Committer)?;
        let author = gitbutler_repo::signature(SignaturePurpose::Author)?;
        let parent = branch.get().peel_to_commit()?;

        let commit_headers = CommitHeadersV2::new();

        let commit_oid = repo.commit_with_signature(
            Some(&branch.try_into()?),
            &author,
            &committer,
            &message,
            &tree,
            &[&parent],
            Some(commit_headers.clone()),
        )?;

        let vb_state = self.ctx.project().virtual_branches();
        // vbranch.head = commit_oid;
        stack.not_in_workspace_wip_change_id = Some(commit_headers.change_id);
        vb_state.set_stack(stack.clone())?;

        Ok(Some(commit_oid))
    }
}
