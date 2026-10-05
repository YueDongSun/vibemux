// Candidate-independent CDP prerequisites. Failure remains a blocked gate.
// A blank target separates renderer availability from candidate defects.

export const browser_preflight_config = Object.freeze({
  timeout_ms: 15_000,
  cleanup_timeout_ms: 3000,
  viewport: Object.freeze({ width: 1280, height: 900 }),
});

export async function preflight_browser_page(connection, { timeout_ms = browser_preflight_config.timeout_ms } = {}) {
  const deadline = performance.now() + timeout_ms;
  let target_id = null;
  let phase = "target_create";
  const send = (method, params = {}, session_id = undefined) => {
    const remaining_ms = Math.floor(deadline - performance.now());
    if (remaining_ms <= 0) throw new Error("browser prerequisite deadline reached");
    return connection.send(method, params, { session_id, timeout_ms: remaining_ms });
  };
  try {
    const created = await send("Target.createTarget", { url: "about:blank" });
    target_id = created.targetId;
    if (typeof target_id !== "string" || target_id.length === 0) throw new Error("missing target identity");
    phase = "target_attach";
    const attached = await send("Target.attachToTarget", { targetId: target_id, flatten: true });
    const session_id = attached.sessionId;
    if (typeof session_id !== "string" || session_id.length === 0) throw new Error("missing session identity");
    for (const domain of ["Page", "Runtime", "DOM", "Network", "Accessibility"]) {
      phase = `${domain.toLowerCase()}_enable`;
      await send(`${domain}.enable`, {}, session_id);
    }
    phase = "emulation_metrics";
    await send("Emulation.setDeviceMetricsOverride", {
      ...browser_preflight_config.viewport, deviceScaleFactor: 1, mobile: false,
    }, session_id);
    phase = "emulation_focus";
    await send("Emulation.setFocusEmulationEnabled", { enabled: true }, session_id);
    phase = "runtime_evaluate";
    const evaluated = await send("Runtime.evaluate", { expression: "1 + 1", returnByValue: true }, session_id);
    if (evaluated.exceptionDetails !== undefined || evaluated.result?.value !== 2) {
      throw new Error("blank renderer evaluation unavailable");
    }
    return null;
  } catch {
    // Fixed, content-free phase; this target has never read candidate content.
    return `browser_page_unavailable:${phase}`;
  } finally {
    if (typeof target_id === "string" && target_id.length > 0) {
      await connection.send("Target.closeTarget", { targetId: target_id }, {
        timeout_ms: browser_preflight_config.cleanup_timeout_ms,
      }).catch(() => undefined);
    }
  }
}
