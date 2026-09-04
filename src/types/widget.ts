export type WidgetPosition = "left" | "right";

export type WidgetConfig = {
  position: WidgetPosition;
  language: "zh" | "en";
};

export const defaultWidgetConfig: WidgetConfig = {
  position: "left",
  language: "zh",
};
