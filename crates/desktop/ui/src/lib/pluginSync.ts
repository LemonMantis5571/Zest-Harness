export const PLUGINS_CHANGED_EVENT = "zest:plugins-changed";

export function notifyPluginsChanged() {
  window.dispatchEvent(new Event(PLUGINS_CHANGED_EVENT));
}
