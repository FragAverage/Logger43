import { useEffect, useMemo } from "react";
import { useCal } from "../stores/calStore";
import { useApp } from "../stores/appStore";
import { TableView } from "../components/TableView";

const VERSIONS = ["430056", "430069"];

export function TablesPage() {
  const cal = useCal();
  const ecu = useApp((s) => s.ecu);

  useEffect(() => {
    cal.init();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const list = useMemo(() => {
    const q = cal.query.trim().toLowerCase();
    return cal.tables
      .filter((t) => !t.constant)
      .filter((t) => !cal.overlayOnly || t.x_channel || t.y_channel)
      .filter((t) => !q || t.title.toLowerCase().includes(q) || t.description.toLowerCase().includes(q) || t.category.toLowerCase().includes(q));
  }, [cal.tables, cal.query, cal.overlayOnly]);

  const calVer = cal.summary?.bin.software_version ?? null;
  const ecuVer = ecu?.software_version ?? null;
  const match = !cal.summary || !ecu ? "none" : !ecuVer ? "unknown" : ecuVer === calVer ? "ok" : "mismatch";

  return (
    <div className="tables-page">
      <div className="cal-bar">
        <button className="primary" onClick={cal.loadBin} disabled={cal.busy}>
          {cal.summary ? "Load other bin…" : "Load calibration bin…"}
        </button>
        {cal.summary && (
          <>
            <span className="mono">
              {cal.summary.bin.software_version} · {cal.summary.bin.layout === "Full512k" ? "512K image" : "64K calibration"}
            </span>
            <span className="muted small mono" title={`${cal.summary.bin.path}\n${cal.summary.xdf_path}`}>
              {basename(cal.summary.bin.path)} + {basename(cal.summary.xdf_path)}
            </span>
            {match === "ok" && <span className="tag ok">matches ECU</span>}
            {match === "mismatch" && <span className="tag bad">ECU runs {ecuVer} — overlay disabled</span>}
            {match === "unknown" && <span className="tag warn">ECU software unknown — check it matches</span>}
            <button className="ghost small" onClick={cal.unload}>
              unload
            </button>
          </>
        )}
        <span className="spacer" />
        <span className="muted small">XDF per version:</span>
        {VERSIONS.map((v) => (
          <button key={v} className="small" title={cal.xdfByVersion[v] ?? "not set"} onClick={() => cal.chooseXdf(v)}>
            {v} {cal.xdfByVersion[v] ? "✓" : "…"}
          </button>
        ))}
      </div>
      {cal.error && <div className="notice bad">{cal.error}</div>}

      {!cal.summary ? (
        <div className="empty">
          Load a 64K calibration or 512K flash image. The matching XDF is chosen by the software id in the bin
          (430056 / 430069). Files are only read, never written.
        </div>
      ) : (
        <div className="tables-split">
          <div className="table-list">
            <input placeholder={`search ${cal.tables.length} tables`} value={cal.query} onChange={(e) => cal.set({ query: e.target.value })} />
            <label className="check small">
              <input type="checkbox" checked={cal.overlayOnly} onChange={(e) => cal.set({ overlayOnly: e.target.checked })} /> only tables with live axes
              ({cal.summary.overlayable})
            </label>
            <div className="table-items">
              {list.slice(0, 500).map((t) => (
                <div
                  key={t.uid}
                  className={`table-item ${cal.selectedUid === t.uid ? "active" : ""}`}
                  onClick={() => cal.select(t.uid)}
                  title={t.description}
                >
                  <div className="mono small">{t.title}</div>
                  <div className="muted small">
                    {t.category} · {t.rows}×{t.cols} · {t.units}
                    {(t.x_channel || t.y_channel) && " · live"}
                  </div>
                </div>
              ))}
              {list.length > 500 && <div className="muted small">… {list.length - 500} more, refine the search</div>}
            </div>
          </div>
          <div className="table-main">
            {cal.table ? <TableView data={cal.table} overlay={match !== "mismatch"} /> : <div className="empty">Pick a table.</div>}
          </div>
        </div>
      )}
    </div>
  );
}

const basename = (p: string) => p.split(/[\\/]/).pop() ?? p;
