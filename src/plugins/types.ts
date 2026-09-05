export type PluginDescriptor = {
  id: string;
  name: string;
  description: string;
  version: string;
  manifestVersion: number;
  permissions: string[];
  enabled: boolean;
  order: number;
  panel?: string | null;
  popup?: string | null;
  settings?: string | null;
  builtin: boolean;
};

export type PluginSurfaceName = "panel" | "popup" | "settings";
