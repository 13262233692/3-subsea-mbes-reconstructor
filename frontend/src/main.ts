import { MbesWsClient } from './ws_client';
import { PointCloudRenderer } from './renderer';
import { SvpRenderer } from './svp_renderer';
import {
  PipelineStatsMsg,
  PointBatchMsg,
  SvpUpdateMsg,
  WelcomeMsg,
  RenderConfig,
  ColorMode,
} from './types';

const DEFAULT_WS_URL = 'ws://localhost:9001';

function qs<T extends HTMLElement = HTMLElement>(sel: string): T {
  const el = document.querySelector<T>(sel);
  if (!el) throw new Error(`Missing element: ${sel}`);
  return el;
}

class App {
  private ws: MbesWsClient;
  private renderer: PointCloudRenderer;
  private svp: SvpRenderer;
  private config: RenderConfig;
  private lastSync: number = 0;
  private hasNewData: boolean = false;
  private rafId: number = 0;

  constructor() {
    const container = qs<HTMLDivElement>('#canvas-container');
    this.config = {
      pointSize: 1.2,
      maxPoints: 1_200_000,
      zScale: 1.0,
      colorMode: 'depth',
      showAxes: false,
      showGrid: true,
    };
    this.ws = new MbesWsClient(DEFAULT_WS_URL, {
      onWelcome: this.onWelcome.bind(this),
      onPoints: this.onPoints.bind(this),
      onSvp: this.onSvp.bind(this),
      onStats: this.onStats.bind(this),
      onConnect: this.onConnect.bind(this),
      onDisconnect: this.onDisconnect.bind(this),
      onError: this.onError.bind(this),
    }, this.config.maxPoints);
    this.renderer = new PointCloudRenderer(container, this.config, {
      onFpsUpdate: (fps, rendered, total) => {
        const el = qs<HTMLSpanElement>('#fps-text');
        el.textContent = `${fps.toFixed(0)} FPS · ${(rendered / 1000).toFixed(0)}K pts`;
      },
    });
    this.svp = new SvpRenderer(qs<HTMLCanvasElement>('#svp-canvas'));
    this._bindUI();
  }

  private _bindUI(): void {
    const ps = qs<HTMLInputElement>('#points-size');
    const psVal = qs<HTMLSpanElement>('#points-size-val');
    ps.addEventListener('input', () => {
      const v = parseFloat(ps.value);
      psVal.textContent = v.toFixed(1);
      this.config.pointSize = v;
      this.renderer.setPointSize(v);
    });
    const pm = qs<HTMLInputElement>('#points-max');
    const pmVal = qs<HTMLSpanElement>('#points-max-val');
    pm.addEventListener('input', () => {
      const v = parseInt(pm.value, 10);
      pmVal.textContent = `${(v / 1000000).toFixed(1)}M`;
      this.config.maxPoints = v;
      this.renderer.setMaxPoints(v);
      this.ws.setMaxPoints(v);
    });
    const zs = qs<HTMLInputElement>('#z-scale');
    const zsVal = qs<HTMLSpanElement>('#z-scale-val');
    zs.addEventListener('input', () => {
      const v = parseFloat(zs.value);
      zsVal.textContent = `${v.toFixed(1)}×`;
      this.config.zScale = v;
      this.renderer.setZScale(v);
      this.hasNewData = true;
    });
    const setMode = (mode: ColorMode) => {
      this.config.colorMode = mode;
      this.renderer.setColorMode(mode);
      (qs('#mode-depth') as HTMLButtonElement).classList.toggle('active', mode === 'depth');
      (qs('#mode-intensity') as HTMLButtonElement).classList.toggle('active', mode === 'intensity');
      (qs('#mode-seep') as HTMLButtonElement).classList.toggle('active', mode === 'seep');
      this.hasNewData = true;
    };
    qs('#mode-depth').addEventListener('click', () => setMode('depth'));
    qs('#mode-intensity').addEventListener('click', () => setMode('intensity'));
    qs('#mode-seep').addEventListener('click', () => setMode('seep'));
    qs('#view-top').addEventListener('click', () => this.renderer.setView('top'));
    qs('#view-iso').addEventListener('click', () => this.renderer.setView('iso'));
    qs('#view-side').addEventListener('click', () => this.renderer.setView('side'));
    const ta = qs<HTMLButtonElement>('#toggle-axes');
    const tg = qs<HTMLButtonElement>('#toggle-grid');
    ta.addEventListener('click', () => {
      this.config.showAxes = !this.config.showAxes;
      this.renderer.setShowAxes(this.config.showAxes);
      ta.classList.toggle('active', this.config.showAxes);
    });
    tg.addEventListener('click', () => {
      this.config.showGrid = !this.config.showGrid;
      this.renderer.setShowGrid(this.config.showGrid);
      tg.classList.toggle('active', this.config.showGrid);
    });
    tg.classList.add('active');
    qs('#clear-points').addEventListener('click', () => {
      this.ws.buffer.clear();
      this.renderer.clearPoints();
      this._setPipelineStats({
        processed_pings: 0, generated_points: 0,
        buffer_flip_count: 0, queued_points: 0, timestamp: 0, type: 'stats',
      });
    });
  }

  private _setWsStatus(connected: boolean, text?: string): void {
    const dot = qs<HTMLSpanElement>('#ws-dot');
    const t = qs<HTMLSpanElement>('#ws-text');
    dot.classList.toggle('ok', connected);
    dot.classList.toggle('warn', !connected);
    t.textContent = text ?? (connected ? '已连接' : '连接中断');
  }

  private _setStreamStatus(active: boolean, text?: string): void {
    const dot = qs<HTMLSpanElement>('#stream-dot');
    const t = qs<HTMLSpanElement>('#stream-text');
    dot.classList.toggle('ok', active);
    dot.classList.toggle('warn', !active);
    t.textContent = text ?? (active ? '数据流正常' : '等待数据');
  }

  private _setPipelineStats(msg: PipelineStatsMsg): void {
    const fmt = (n: number) => {
      if (n >= 1e9) return (n / 1e9).toFixed(2) + 'B';
      if (n >= 1e6) return (n / 1e6).toFixed(2) + 'M';
      if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K';
      return n.toFixed(0);
    };
    qs<HTMLSpanElement>('#stat-pings').textContent = fmt(msg.processed_pings);
    qs<HTMLSpanElement>('#stat-points').textContent = fmt(msg.generated_points);
    qs<HTMLSpanElement>('#stat-flips').textContent = fmt(msg.buffer_flip_count);
    qs<HTMLSpanElement>('#stat-queue').textContent = fmt(msg.queued_points);
  }

  start(): void {
    this.renderer.start();
    this.ws.connect();
    const tick = () => {
      const now = performance.now();
      if (this.hasNewData || (now - this.lastSync) > 70) {
        this.renderer.forceSync(this.ws.buffer);
        this.lastSync = now;
        this.hasNewData = false;
      }
      this.rafId = requestAnimationFrame(tick);
    };
    this.rafId = requestAnimationFrame(tick);
  }

  private onWelcome(msg: WelcomeMsg): void {
    console.info('[WS] Connected to MBES server', msg);
    this._setWsStatus(true, `连接 v${msg.server_version}`);
  }

  private onPoints(msg: PointBatchMsg): void {
    this.hasNewData = true;
    this._setStreamStatus(true, `Ping ${msg.ping_start}-${msg.ping_end}`);
    qs<HTMLSpanElement>('#stat-batch').textContent = msg.batch_id.toString();
    qs<HTMLSpanElement>('#stat-pingrange').textContent = `${msg.ping_start}-${msg.ping_end}`;
    qs<HTMLSpanElement>('#stat-depth').textContent = `${msg.depth_min.toFixed(1)}-${msg.depth_max.toFixed(1)}m`;
    if (msg.points_flat.length > 0) {
      const stride = 7;
      let seepSum = 0;
      let seepCount = 0;
      for (let i = 5; i < msg.points_flat.length; i += stride) {
        if (msg.points_flat[i] > 0.2) {
          seepSum += msg.points_flat[i];
          seepCount++;
        }
      }
      const seepEl = qs<HTMLSpanElement>('#stat-seep');
      if (seepCount > 0) {
        seepEl.innerHTML = `<span class="seep-indicator">检测到 ${(seepCount/1000).toFixed(1)}K 点</span>`;
      } else {
        seepEl.innerHTML = `<span class="seep-indicator" style="opacity:0.5">监测中</span>`;
      }
    }
  }

  private onSvp(msg: SvpUpdateMsg): void {
    console.info('[WS] Received SVP update:', msg.layer_count, 'layers');
    this.svp.updateLayers(msg.depths, msg.velocities);
    qs<HTMLSpanElement>('#svp-info').textContent = this.svp.getStats();
  }

  private onStats(msg: PipelineStatsMsg): void {
    this._setPipelineStats(msg);
    this._setStreamStatus(msg.processed_pings > 0, msg.queued_points > 0 ? `队列 ${msg.queued_points}` : '空闲');
  }

  private onConnect(): void {
    this._setWsStatus(true, 'WebSocket 已连接');
  }

  private onDisconnect(): void {
    this._setWsStatus(false, '连接中断 · 重连中');
    this._setStreamStatus(false, '等待数据流');
  }

  private onError(err: Error): void {
    console.warn('[WS] error:', err);
  }
}

function boot(): void {
  const app = new App();
  app.start();
  (window as any).__app = app;
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', boot);
} else {
  boot();
}
