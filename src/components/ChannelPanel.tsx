import { useMemo } from "react";
import { useApp } from "../stores/appStore";
import type { ChannelInfo } from "../types/ms43";

const VER_TITLE = {
  CrossChecked: "BMW SGBD + real MS43 capture + independent tool. Pending test on your car.",
  Sgbd: "BMW MS430DS0 SGBD only; not yet seen in a real capture.",
  Community: "Community RAM address (RomRaider/MS4X), DME-software specific, unverified.",
};

export function ChannelPanel() {
  const { channels, presets, selected, activePreset, select, toggleChannel, plan, status } = useApp();
  const locked = status.state === "Logging";
  const unavailable = useMemo(() => new Map(plan?.unavailable.map((u) => [u.id, u.reason]) ?? []), [plan]);

  const groups = useMemo(() => {
    const m = new Map<string, ChannelInfo[]>();
    for (const c of channels) {
      if (!m.has(c.group)) m.set(c.group, []);
      m.get(c.group)!.push(c);
    }
    return [...m.entries()];
  }, [channels]);

  return (
    <aside className="left">
      <h2>Presets</h2>
      <div className="presets">
        {presets.map((p) => (
          <button
            key={p.id}
            className={activePreset === p.id ? "active" : ""}
            title={p.description}
            disabled={locked}
            onClick={() => select([...p.channels], p.id)}
          >
            {p.name}
          </button>
        ))}
      </div>

      {plan && plan.requests.length > 0 && (
        <div className="plan">
          <h2>Requests per cycle</h2>
          {plan.requests.map((r) => (
            <div key={r.frame} className="plan-row" title={r.channels.join(", ")}>
              <span className="mono">{r.frame}</span>
              <span className="muted">{r.every_n_cycles === 1 ? "every cycle" : `1/${r.every_n_cycles}`}</span>
            </div>
          ))}
        </div>
      )}

      <h2>
        Channels <span className="muted">({selected.length})</span>
      </h2>
      <div className="channel-list">
        {groups.map(([group, list]) => (
          <div key={group} className="group">
            <div className="group-name">{group}</div>
            {list.map((c) => {
              const why = unavailable.get(c.id);
              return (
                <label
                  key={c.id}
                  className={`ch ${why ? "unavail" : ""}`}
                  title={[c.source_text, c.sources, c.note, why && `UNAVAILABLE: ${why}`].filter(Boolean).join("\n")}
                >
                  <input type="checkbox" checked={selected.includes(c.id)} disabled={locked} onChange={() => toggleChannel(c.id)} />
                  <span className="ch-label">{c.label}</span>
                  <span className={`prio prio-${c.priority}`}>{c.priority[0]}</span>
                  <span className={`dot ${c.verification}`} title={VER_TITLE[c.verification]} />
                </label>
              );
            })}
          </div>
        ))}
      </div>
      <div className="legend small muted">
        <span><i className="dot CrossChecked" /> cross-checked</span>
        <span><i className="dot Sgbd" /> BMW SGBD</span>
        <span><i className="dot Community" /> community</span>
      </div>
    </aside>
  );
}
