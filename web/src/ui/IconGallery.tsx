import { iconRegistry, DisclosureIcon, DiffMarkIcon } from "./icons";

export function IconGallery() {
  return (
    <div style={{ padding: 24, fontFamily: "system-ui, sans-serif" }}>
      <h1 style={{ fontSize: 18, fontWeight: 600, marginBottom: 8 }}>Icons — 16 and 24</h1>
      <p style={{ opacity: 0.6, marginBottom: 20, fontSize: 13 }}>
        Outline 1.5 on 16-unit grid, round caps/joins, currentColor, 12×14 centred.
      </p>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(180px, 1fr))", gap: 12 }}>
        {iconRegistry.map(({ name, Component }) => (
          <div key={name} style={{ border: "1px solid #e8e8e8", borderRadius: 10, padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
            <span style={{ fontSize: 11, fontWeight: 600, opacity: 0.7 }}>{name}</span>
            <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
              <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}>
                <Component size={16} /> 16
              </span>
              <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}>
                <Component size={24} /> 24
              </span>
            </div>
          </div>
        ))}
        {/* parametric icons */}
        <div style={{ border: "1px solid #e8e8e8", borderRadius: 10, padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          <span style={{ fontSize: 11, fontWeight: 600, opacity: 0.7 }}>DisclosureIcon (closed/open)</span>
          <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}><DisclosureIcon open={false} size={16} /> 16</span>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}><DisclosureIcon open={true} size={16} /> open</span>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}><DisclosureIcon open={false} size={24} /> 24</span>
          </div>
        </div>
        <div style={{ border: "1px solid #e8e8e8", borderRadius: 10, padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          <span style={{ fontSize: 11, fontWeight: 600, opacity: 0.7 }}>DiffMarkIcon (added/removed)</span>
          <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}><DiffMarkIcon kind="added" size={16} /> added</span>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontSize: 12 }}><DiffMarkIcon kind="removed" size={16} /> removed</span>
          </div>
        </div>
      </div>
    </div>
  );
}
