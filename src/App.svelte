<script lang="ts">
  import Sidebar from "./lib/Sidebar.svelte";
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
      <div class="terminal-placeholder">
        Terminal for session: {activeSession}
      </div>
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
    align-items: center;
    justify-content: center;
    background: #11111b;
    color: #cdd6f4;
  }

  .terminal-placeholder {
    font-size: 14px;
    color: #cdd6f4;
  }

  .empty-state {
    font-size: 14px;
    color: #6c7086;
  }
</style>
