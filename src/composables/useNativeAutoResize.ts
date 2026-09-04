import { invoke } from "@tauri-apps/api/core";
import { nextTick, onUnmounted, type Ref } from "vue";

type NativeSize = { width: number; height: number };
type MeasureSize = (element: HTMLElement) => NativeSize;

type NativeAutoResizeOptions = {
  command: string;
  measure?: MeasureSize;
  extraArgs?: Record<string, unknown>;
};

const defaultMeasure: MeasureSize = element => ({
  width: Math.ceil(Math.max(element.getBoundingClientRect().width, element.scrollWidth)),
  height: Math.ceil(Math.max(element.getBoundingClientRect().height, element.scrollHeight)),
});

/**
 * DOM 尺寸变化 -> Tauri 原生窗口尺寸同步。
 * 使用 rAF 合并同一帧内的多次 ResizeObserver 回调，并跳过重复尺寸。
 */
export function useNativeAutoResize(target: Ref<HTMLElement | null>, options: NativeAutoResizeOptions) {
  let observer: ResizeObserver | undefined;
  let frame = 0;
  let lastSize = "";
  let disposed = false;

  async function resize(force = false) {
    await nextTick();
    if (disposed) return;
    const element = target.value;
    if (!element) return;
    const measured = (options.measure ?? defaultMeasure)(element);
    const width = Math.max(1, Math.ceil(measured.width));
    const height = Math.max(1, Math.ceil(measured.height));
    const key = `${width}x${height}`;
    if (!force && key === lastSize) return;
    lastSize = key;
    await invoke(options.command, { ...(options.extraArgs ?? {}), width, height }).catch(() => undefined);
  }

  function schedule(force = false) {
    if (disposed) return;
    if (force) lastSize = "";
    if (frame) cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      frame = 0;
      void resize(force);
    });
  }

  function start() {
    if (observer) return;
    observer = new ResizeObserver(() => schedule());
    if (target.value) observer.observe(target.value);
    schedule(true);
  }

  function stop() {
    disposed = true;
    observer?.disconnect();
    observer = undefined;
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
  }

  onUnmounted(stop);
  return { start, stop, resize, schedule };
}
