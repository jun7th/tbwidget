const params = new URLSearchParams(window.location.search);

export const windowMode = {
  settings: params.has("settings"),
  pluginPopup: params.has("pluginPopup"),
  pluginSettings: params.has("pluginSettings"),
};

export const isTaskbarWindow = !windowMode.settings && !windowMode.pluginPopup && !windowMode.pluginSettings;
