import { ServerMessage, PointBatchMsg, SvpUpdateMsg, PipelineStatsMsg, WelcomeMsg } from './types';

export type WsEventHandler = {
  onWelcome?: (msg: WelcomeMsg) => void;
  onPoints?: (msg: PointBatchMsg) => void;
  onSvp?: (msg: SvpUpdateMsg) => void;
  onStats?: (msg: PipelineStatsMsg) => void;
  onConnect?: () => void;
  onDisconnect?: () => void;
  onError?: (err: Error) => void;
};

const STRIDE = 7;

export class PointCloudBuffer {
  private maxSize: number;
  private x: Float32Array;
  private y: Float32Array;
  private z: Float32Array;
  private intensity: Float32Array;
  private reflectivity: Float32Array;
  private seepHint: Float32Array;
  private quality: Float32Array;
  private pingNumber: Float32Array;
  private writeIdx: number;
  private totalWritten: number;

  constructor(maxSize: number) {
    this.maxSize = maxSize;
    this.x = new Float32Array(maxSize);
    this.y = new Float32Array(maxSize);
    this.z = new Float32Array(maxSize);
    this.intensity = new Float32Array(maxSize);
    this.reflectivity = new Float32Array(maxSize);
    this.seepHint = new Float32Array(maxSize);
    this.quality = new Float32Array(maxSize);
    this.pingNumber = new Float32Array(maxSize);
    this.writeIdx = 0;
    this.totalWritten = 0;
  }

  resize(newMax: number): void {
    if (newMax === this.maxSize) return;
    const copyCount = Math.min(this.writeIdx, newMax);
    const nx = new Float32Array(newMax);
    const ny = new Float32Array(newMax);
    const nz = new Float32Array(newMax);
    const ni = new Float32Array(newMax);
    const nr = new Float32Array(newMax);
    const ns = new Float32Array(newMax);
    const nq = new Float32Array(newMax);
    const np = new Float32Array(newMax);
    const startSrc = this.writeIdx > copyCount ? this.writeIdx - copyCount : 0;
    for (let i = 0; i < copyCount; i++) {
      const si = (startSrc + i) % this.maxSize;
      nx[i] = this.x[si];
      ny[i] = this.y[si];
      nz[i] = this.z[si];
      ni[i] = this.intensity[si];
      nr[i] = this.reflectivity[si];
      ns[i] = this.seepHint[si];
      nq[i] = this.quality[si];
      np[i] = this.pingNumber[si];
    }
    this.x = nx; this.y = ny; this.z = nz;
    this.intensity = ni; this.reflectivity = nr;
    this.seepHint = ns; this.quality = nq; this.pingNumber = np;
    this.maxSize = newMax;
    this.writeIdx = copyCount;
  }

  addBatch(msg: PointBatchMsg): number {
    const flat = msg.points_flat;
    const count = msg.point_count;
    const expectedLen = count * STRIDE;
    const actualCount = Math.min(count, Math.floor(flat.length / STRIDE));
    for (let i = 0; i < actualCount; i++) {
      const si = i * STRIDE;
      const di = (this.writeIdx + i) % this.maxSize;
      this.x[di] = flat[si];
      this.y[di] = flat[si + 1];
      this.z[di] = flat[si + 2];
      this.intensity[di] = flat[si + 3];
      this.reflectivity[di] = flat[si + 4];
      this.seepHint[di] = flat[si + 5];
      this.quality[di] = flat[si + 6];
      this.pingNumber[di] = (msg.ping_start + (i / actualCount) * (msg.ping_end - msg.ping_start)) || 0;
    }
    this.writeIdx = (this.writeIdx + actualCount) % this.maxSize;
    this.totalWritten += actualCount;
    return actualCount;
  }

  clear(): void {
    this.writeIdx = 0;
    this.totalWritten = 0;
  }

  get filledCount(): number {
    return this.totalWritten < this.maxSize ? this.writeIdx : this.maxSize;
  }

  get total(): number { return this.totalWritten; }
  get capacity(): number { return this.maxSize; }
  get X(): Float32Array { return this.x; }
  get Y(): Float32Array { return this.y; }
  get Z(): Float32Array { return this.z; }
  get I(): Float32Array { return this.intensity; }
  get R(): Float32Array { return this.reflectivity; }
  get S(): Float32Array { return this.seepHint; }
  get Q(): Float32Array { return this.quality; }
  get P(): Float32Array { return this.pingNumber; }
}

export class MbesWsClient {
  private url: string;
  private ws: WebSocket | null = null;
  private handlers: WsEventHandler;
  private reconnectTimer: number | null = null;
  private reconnectDelay: number;
  private shouldReconnect: boolean;
  public readonly buffer: PointCloudBuffer;

  constructor(url: string, handlers: WsEventHandler, maxPoints: number = 1_200_000) {
    this.url = url;
    this.handlers = handlers;
    this.reconnectDelay = 2000;
    this.shouldReconnect = true;
    this.buffer = new PointCloudBuffer(maxPoints);
  }

  connect(): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) return;
    try {
      this.ws = new WebSocket(this.url);
    } catch (e) {
      this.handlers.onError?.(e as Error);
      this.scheduleReconnect();
      return;
    }
    this.ws.onopen = () => {
      this.reconnectDelay = 1000;
      this.handlers.onConnect?.();
    };
    this.ws.onmessage = (ev) => {
      try {
        const data: ServerMessage = JSON.parse(ev.data);
        this.dispatch(data);
      } catch (e) {
        this.handlers.onError?.(e as Error);
      }
    };
    this.ws.onerror = (ev) => {
      const err = new Error('WebSocket error');
      (err as any).event = ev;
      this.handlers.onError?.(err);
    };
    this.ws.onclose = () => {
      this.handlers.onDisconnect?.();
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect(): void {
    if (!this.shouldReconnect) return;
    if (this.reconnectTimer !== null) return;
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = null;
      this.reconnectDelay = Math.min(this.reconnectDelay * 1.5, 10000);
      this.connect();
    }, this.reconnectDelay);
  }

  disconnect(): void {
    this.shouldReconnect = false;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  setMaxPoints(n: number): void {
    this.buffer.resize(n);
  }

  private dispatch(msg: ServerMessage): void {
    switch (msg.type) {
      case 'welcome':
        this.handlers.onWelcome?.(msg);
        break;
      case 'points':
        this.buffer.addBatch(msg);
        this.handlers.onPoints?.(msg);
        break;
      case 'svp':
        this.handlers.onSvp?.(msg);
        break;
      case 'stats':
        this.handlers.onStats?.(msg);
        break;
    }
  }
}
