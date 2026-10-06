import { useEffect } from "react";
import { listenEngine } from "./lib/api";
import { samples } from "./stores/sampleStore";
import { useApp } from "./stores/appStore";
import { TopBar } from "./components/TopBar";
import { ChannelPanel } from "./components/ChannelPanel";
import { LogPanel } from "./components/LogPanel";
import { LivePage } from "./pages/LivePage";
import { DebugPage } from "./pages/DebugPage";
import { TablesPage } from "./pages/TablesPage";

export function App() {
  const page = useApp((s) => s.page);

  useEffect(() => {
    const st = useApp.getState();
    st.init().catch((e) => st.set({ lastError: String(e) }));
    let off: (() => void) | undefined;
    let cancelled = false;
    listenEngine({
      connection_status: (p) => {
        useApp.getState().set({ status: p });
        if (p.state === "Disconnected") useApp.getState().set({ stats: null });
      },
      ecu_info: (p) => useApp.getState().set({ ecu: p }),
      sample_batch: (p) => samples.append(p),
      logger_stats: (p) => useApp.getState().set({ stats: p, plan: p.plan ?? useApp.getState().plan }),
      protocol_debug: (p) => useApp.getState().pushDebug(p),
      protocol_error: (p) => useApp.getState().pushError(p),
    }).then((u) => {
      if (cancelled) u();
      else off = u;
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  return (
    <div className="app">
      <TopBar />
      <ChannelPanel />
      <main>{page === "live" ? <LivePage /> : page === "tables" ? <TablesPage /> : <DebugPage />}</main>
      <LogPanel />
    </div>
  );
}
