<script lang="ts">
  import Terminal from "./Terminal.svelte";
  import { projects, activeSessionId, hotkeyAction, type Project } from "./stores";

  let projectList: Project[] = $state([]);
  let activeSession: string | null = $state(null);
  let terminalComponents: Record<string, Terminal> = $state({});

  projects.subscribe((value) => {
    projectList = value;
  });

  activeSessionId.subscribe((value) => {
    activeSession = value;
  });

  hotkeyAction.subscribe((action) => {
    if (action?.type === "focus-terminal" && activeSession) {
      terminalComponents[activeSession]?.focus();
    }
  });

  let allSessionIds: string[] = $derived(
    projectList.flatMap((p) => p.sessions.map((s) => s.id)),
  );
</script>

<div class="terminal-manager">
  {#each allSessionIds as sessionId (sessionId)}
    <div class="terminal-wrapper" class:visible={activeSession === sessionId}>
      <Terminal {sessionId} bind:this={terminalComponents[sessionId]} />
    </div>
  {/each}

  {#if !activeSession}
    <div class="empty-state">Select or create a session to begin.</div>
  {/if}
</div>

<style>
  .terminal-manager {
    width: 100%;
    height: 100%;
    position: relative;
  }

  .terminal-wrapper {
    position: absolute;
    inset: 0;
    display: none;
  }

  .terminal-wrapper.visible {
    display: block;
  }

  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: #6c7086;
    font-size: 14px;
  }
</style>
