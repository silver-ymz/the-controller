<script lang="ts">
  import Sidebar from "./lib/Sidebar.svelte";
  import Terminal from "./lib/Terminal.svelte";
  import { activeSessionId } from "./lib/stores";

  let activeSession: string | null = $state(null);

  activeSessionId.subscribe((value) => {
    activeSession = value;
  });
</script>

<div class="app-layout">
  <Sidebar />
  <main class="terminal-area">
    {#if activeSession}
      {#key activeSession}
        <Terminal sessionId={activeSession} />
      {/key}
    {:else}
      <div class="empty-state">
        Select or create a session to begin.
      </div>
    {/if}
  </main>
</div>

<style>
  .app-layout {
    display: flex;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
  }

  .terminal-area {
    flex: 1;
    display: flex;
    background: #11111b;
    color: #cdd6f4;
    overflow: hidden;
  }

  .empty-state {
    font-size: 14px;
    color: #6c7086;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
  }
</style>
