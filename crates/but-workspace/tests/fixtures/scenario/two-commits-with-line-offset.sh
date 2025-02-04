#!/usr/bin/env bash

### Description
# A single branch with two commits. The first commit has 100 lines, the second adds
# another 30 lines to the top of the file.
# A local worktree change affects the bottom-most 20 lines.
# Large numbers are used make fuzzy-patch application harder.
set -eu -o pipefail

git init
seq 100 >file
git add . && git commit -m init && git tag first-commit

{ seq 20 40; seq 100; } >file && git commit -am "insert 20 lines to the top"

# change the last 20 lines
{ seq 20 40; seq 80; seq 30 50; } >file

