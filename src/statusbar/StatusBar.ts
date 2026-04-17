// Bottom status bar: shows real-time CPU / RAM / GPU / Disk / Network stats
// streamed from the Rust sysmonitor thread via a Tauri event.

import { listen as tauriListen } from "@tauri-apps/api/event";

interface GpuInfo {
  name: string;
  usage: number;
}

interface DiskInfo {
  name: string;
  total_gb: number;
  used_gb: number;
  usage: number;
}

interface NetInfo {
  upload_bytes_sec: number;
  download_bytes_sec: number;
}

interface SystemSnapshot {
  cpu_usage: number;
  ram_total_mb: number;
  ram_used_mb: number;
  ram_usage: number;
  gpus: GpuInfo[];
  disks: DiskInfo[];
  net: NetInfo;
}

function formatBytes(bytesPerSec: number): string {
  if (bytesPerSec < 1024) return `${bytesPerSec} B/s`;
  if (bytesPerSec < 1024 * 1024) return `${(bytesPerSec / 1024).toFixed(1)} KB/s`;
  return `${(bytesPerSec / (1024 * 1024)).toFixed(1)} MB/s`;
}

function usageColor(pct: number): string {
  if (pct >= 90) return "var(--status-critical)";
  if (pct >= 70) return "var(--status-warn)";
  return "var(--status-ok)";
}

export async function mountStatusBar(parent: HTMLElement): Promise<() => void> {
  const bar = document.createElement("div");
  bar.className = "status-bar";

  const cpuEl = makeSegment("CPU", "—");
  const ramEl = makeSegment("RAM", "—");
  const gpuEl = makeSegment("GPU", "—");
  const diskEl = makeSegment("DISK", "—");
  const netUpEl = makeSegment("↑", "—");
  const netDownEl = makeSegment("↓", "—");

  bar.appendChild(cpuEl.root);
  bar.appendChild(ramEl.root);
  bar.appendChild(gpuEl.root);
  bar.appendChild(diskEl.root);
  bar.appendChild(makeSep());
  bar.appendChild(netUpEl.root);
  bar.appendChild(netDownEl.root);

  parent.appendChild(bar);

  const unlisten = await tauriListen<SystemSnapshot>(
    "app:sysmonitor",
    (ev) => {
      const s = ev.payload;

      cpuEl.update(`${s.cpu_usage.toFixed(0)}%`, usageColor(s.cpu_usage));
      ramEl.update(
        `${s.ram_used_mb.toLocaleString()}/${s.ram_total_mb.toLocaleString()} MB (${s.ram_usage.toFixed(0)}%)`,
        usageColor(s.ram_usage),
      );

      if (s.gpus.length === 0) {
        gpuEl.update("N/A", "var(--fg-muted)");
      } else {
        const text = s.gpus
          .map((g, i) =>
            s.gpus.length > 1
              ? `GPU${i}: ${g.usage.toFixed(0)}%`
              : `${g.usage.toFixed(0)}%`,
          )
          .join("  ");
        const maxUsage = Math.max(...s.gpus.map((g) => g.usage));
        gpuEl.update(text, usageColor(maxUsage));
        gpuEl.root.title = s.gpus.map((g) => `${g.name}: ${g.usage.toFixed(1)}%`).join("\n");
      }

      if (s.disks.length === 0) {
        diskEl.update("N/A", "var(--fg-muted)");
      } else if (s.disks.length <= 3) {
        const text = s.disks
          .map((d) => `${d.name} ${d.usage.toFixed(0)}%`)
          .join("  ");
        const maxUsage = Math.max(...s.disks.map((d) => d.usage));
        diskEl.update(text, usageColor(maxUsage));
        diskEl.root.title = s.disks
          .map((d) => `${d.name} ${d.used_gb}/${d.total_gb} GB (${d.usage.toFixed(1)}%)`)
          .join("\n");
      } else {
        const maxUsage = Math.max(...s.disks.map((d) => d.usage));
        diskEl.update(`${s.disks.length} disks — max ${maxUsage.toFixed(0)}%`, usageColor(maxUsage));
        diskEl.root.title = s.disks
          .map((d) => `${d.name} ${d.used_gb}/${d.total_gb} GB (${d.usage.toFixed(1)}%)`)
          .join("\n");
      }

      netUpEl.update(formatBytes(s.net.upload_bytes_sec), "var(--fg)");
      netDownEl.update(formatBytes(s.net.download_bytes_sec), "var(--fg)");
    },
  );

  return () => {
    unlisten();
    bar.remove();
  };
}

interface SegmentHandle {
  root: HTMLElement;
  update: (text: string, color?: string) => void;
}

function makeSegment(label: string, initial: string): SegmentHandle {
  const el = document.createElement("div");
  el.className = "status-bar__seg";

  const lbl = document.createElement("span");
  lbl.className = "status-bar__label";
  lbl.textContent = label;

  const val = document.createElement("span");
  val.className = "status-bar__value";
  val.textContent = initial;

  el.appendChild(lbl);
  el.appendChild(val);

  return {
    root: el,
    update(text: string, color?: string) {
      val.textContent = text;
      if (color) val.style.color = color;
    },
  };
}

function makeSep(): HTMLElement {
  const sep = document.createElement("div");
  sep.className = "status-bar__sep";
  return sep;
}
