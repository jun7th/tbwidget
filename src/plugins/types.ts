export type PluginBundle = {
  id: string;
  name: string;
  description: string;
  version: string;
  manifestVersion: number;
  permissions: string[];
  enabled: boolean;
  order: number;
  panelHtml: string;
  panelJs: string;
  popupHtml?: string | null;
  popupJs?: string | null;
  settingsJs?: string | null;
  wasmBytes?: number[] | null;
  builtin: boolean;
};
