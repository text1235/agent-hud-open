/** A single visible HUD owns its alerts even when the pointer is elsewhere. */
export function singleAlertDisplay(
  screenId: string,
  monitors: { screenId: string; primary: boolean }[],
  settings: { hudScreenId?: string; hudPrimaryOnly?: boolean },
): boolean | undefined {
  if (!settings.hudScreenId && !settings.hudPrimaryOnly) return undefined;
  const selected =
    monitors.find((monitor) => monitor.screenId === settings.hudScreenId) ??
    monitors.find((monitor) => monitor.primary) ??
    monitors[0];
  return selected ? selected.screenId === screenId : undefined;
}
