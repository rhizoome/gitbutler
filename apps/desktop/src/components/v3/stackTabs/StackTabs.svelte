<script lang="ts">
	import ReduxResult from '$components/ReduxResult.svelte';
	import StackTab from '$components/v3/stackTabs/StackTab.svelte';
	import StackTabNew from '$components/v3/stackTabs/StackTabNew.svelte';
	import { stackPath } from '$lib/routes/routes.svelte';
	import { StackService } from '$lib/stacks/stackService.svelte';
	import { inject } from '@gitbutler/shared/context';
	import { onMount } from 'svelte';

	type Props = {
		projectId: string;
		selectedId?: string;
		previewing?: boolean;
		width: number | undefined;
	};
	let { projectId, selectedId, width = $bindable() }: Props = $props();

	const [stackService] = inject(StackService);
	const result = $derived(stackService.stacks(projectId));

	let plusBtnEl = $state<HTMLButtonElement>();
	let tabsEl = $state<HTMLDivElement>();
	let scrollerEl = $state<HTMLDivElement>();

	let scrollable = $state(false);
	let scrolled = $state(false);
	let scrolledEnd = $state(false);

	function onscroll() {
		scrolled = scrollerEl && scrollerEl.scrollLeft > 0 ? true : false;
		scrolledEnd = scrollerEl
			? scrollerEl.scrollLeft + scrollerEl.offsetWidth >= scrollerEl.scrollWidth
			: false;
	}

	onMount(() => {
		const observer = new ResizeObserver(() => {
			scrollable = scrollerEl ? scrollerEl.scrollWidth > scrollerEl.offsetWidth : false;
			width = tabsEl?.offsetWidth;
		});
		observer.observe(tabsEl!);
		return () => {
			observer.disconnect();
		};
	});
</script>

<div class="tabs" bind:this={tabsEl}>
	{#if (result.current.data?.length ?? 1) > 0}
		<div class="inner">
			<div class="scroller" bind:this={scrollerEl} class:scrolled {onscroll}>
				<ReduxResult {projectId} result={result.current}>
					{#snippet children(result, env)}
						{#each result as tab, i (tab.branchNames[0])}
							{@const first = i === 0}
							{@const last = i === result.length - 1}
							{@const selected = tab.id === selectedId}

							<StackTab
								name={tab.branchNames[0]!}
								projectId={env.projectId}
								stackId={tab.id}
								href={stackPath(env.projectId, tab.id)}
								anchors={tab.branchNames.slice(1)}
								{selected}
								onNextTab={() => {
									if (last) {
										plusBtnEl?.focus();
									}
								}}
								onPrevTab={() => {
									if (first) {
										plusBtnEl?.focus();
									}
								}}
							/>
						{/each}
					{/snippet}
				</ReduxResult>
			</div>
			<div class="shadow shadow-left" class:scrolled></div>
			<div class="shadow shadow-right" class:scrollable class:scrolled-end={scrolledEnd}></div>
		</div>
	{/if}
	<StackTabNew bind:el={plusBtnEl} {scrollerEl} {projectId} stackId={selectedId} />
</div>

<style lang="postcss">
	.tabs {
		display: flex;
		max-width: 100%;
		width: fit-content;
	}

	.scroller {
		display: flex;
		overflow-x: scroll;
		scroll-snap-type: x proximity;
		scroll-behavior: smooth;
	}

	.scroller::-webkit-scrollbar {
		display: none;
	}

	.inner {
		position: relative;
		overflow-x: hidden;
		border-radius: 10px 0 0 0;
		border-left: 1px solid var(--clr-border-2);
		border-top: 1px solid var(--clr-border-2);
	}

	.shadow {
		position: absolute;
		top: 0;
		height: 100%;
		width: 12px;
	}

	.shadow-left {
		pointer-events: none;
		opacity: 0;
		left: 0;
		background: linear-gradient(
			to right,
			var(--clr-bg-3) 0%,
			oklch(from var(--clr-bg-3) l c h / 0) 100%
		);
		transition: opacity var(--transition-fast);

		&.scrolled {
			opacity: 1;
		}
	}

	.shadow-right {
		pointer-events: none;
		opacity: 0;
		right: 0;
		background: linear-gradient(
			to left,
			var(--clr-bg-3) 0%,
			oklch(from var(--clr-bg-3) l c h / 0) 100%
		);
		transition: opacity var(--transition-fast);

		&.scrollable {
			opacity: 1;

			&.scrolled-end {
				opacity: 0;
			}
		}
	}
</style>
