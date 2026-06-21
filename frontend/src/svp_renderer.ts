import { SvpLayer } from './types';

export class SvpRenderer {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private layers: SvpLayer[] = [];
  private dpr: number;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d')!;
    this.dpr = window.devicePixelRatio || 1;
    this.resize();
    window.addEventListener('resize', () => this.resize());
  }

  resize(): void {
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    this.canvas.width = Math.floor(w * this.dpr);
    this.canvas.height = Math.floor(h * this.dpr);
    this.ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    this.render();
  }

  updateLayers(depths: number[], velocities: number[]): void {
    this.layers = depths.map((d, i) => ({ depth: d, velocity: velocities[i] }));
    this.render();
  }

  getStats(): string {
    if (this.layers.length === 0) return '等待声速剖面数据...';
    const ds = this.layers.map(l => l.depth);
    const vs = this.layers.map(l => l.velocity);
    const dMin = Math.min(...ds);
    const dMax = Math.max(...ds);
    const vMin = Math.min(...vs);
    const vMax = Math.max(...vs);
    return `${this.layers.length}层 · 深度 ${dMin.toFixed(0)}-${dMax.toFixed(0)}m · 声速 ${vMin.toFixed(1)}-${vMax.toFixed(1)} m/s`;
  }

  private render(): void {
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    const ctx = this.ctx;
    ctx.clearRect(0, 0, w, h);
    const paddingL = 44, paddingR = 10, paddingT = 10, paddingB = 22;
    const pw = w - paddingL - paddingR;
    const ph = h - paddingT - paddingB;
    if (this.layers.length < 2) {
      ctx.strokeStyle = 'rgba(148, 163, 184, 0.3)';
      ctx.setLineDash([4, 3]);
      ctx.beginPath();
      ctx.moveTo(paddingL, paddingT);
      ctx.lineTo(paddingL, h - paddingB);
      ctx.moveTo(paddingL, h - paddingB);
      ctx.lineTo(w - paddingR, h - paddingB);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = 'rgba(148, 163, 184, 0.6)';
      ctx.font = '11px -apple-system, sans-serif';
      ctx.textAlign = 'center';
      ctx.fillText('等待 SVP 数据...', w / 2, h / 2);
      return;
    }
    const depths = this.layers.map(l => l.depth);
    const velocities = this.layers.map(l => l.velocity);
    const dMin = Math.min(...depths);
    const dMax = Math.max(...depths);
    let vMin = Math.min(...velocities);
    let vMax = Math.max(...velocities);
    const vPad = Math.max(5, (vMax - vMin) * 0.1);
    vMin -= vPad; vMax += vPad;
    const dRange = Math.max(1, dMax - dMin);
    const vRange = Math.max(0.001, vMax - vMin);
    const xForV = (v: number) => paddingL + ((v - vMin) / vRange) * pw;
    const yForD = (d: number) => paddingT + ((d - dMin) / dRange) * ph;
    ctx.fillStyle = 'rgba(148, 163, 184, 0.15)';
    ctx.strokeStyle = 'rgba(148, 163, 184, 0.3)';
    ctx.lineWidth = 1;
    const vTicks = 5;
    for (let i = 0; i <= vTicks; i++) {
      const v = vMin + (vRange * i) / vTicks;
      const x = xForV(v);
      ctx.beginPath();
      ctx.moveTo(x, paddingT);
      ctx.lineTo(x, h - paddingB);
      ctx.stroke();
      ctx.fillStyle = 'rgba(148, 163, 184, 0.75)';
      ctx.font = '10px monospace';
      ctx.textAlign = 'center';
      ctx.fillText(v.toFixed(0), x, h - paddingB + 14);
    }
    const dTicks = 5;
    for (let i = 0; i <= dTicks; i++) {
      const d = dMin + (dRange * i) / dTicks;
      const y = yForD(d);
      ctx.strokeStyle = 'rgba(148, 163, 184, 0.2)';
      ctx.beginPath();
      ctx.moveTo(paddingL, y);
      ctx.lineTo(w - paddingR, y);
      ctx.stroke();
      ctx.fillStyle = 'rgba(148, 163, 184, 0.75)';
      ctx.font = '10px monospace';
      ctx.textAlign = 'right';
      ctx.textBaseline = 'middle';
      ctx.fillText(`${d.toFixed(0)}m`, paddingL - 4, y);
    }
    ctx.textBaseline = 'alphabetic';
    const grad = ctx.createLinearGradient(0, paddingT, 0, h - paddingB);
    grad.addColorStop(0, 'rgba(56, 189, 248, 0.05)');
    grad.addColorStop(0.5, 'rgba(56, 189, 248, 0.2)');
    grad.addColorStop(1, 'rgba(56, 189, 248, 0.4)');
    ctx.beginPath();
    this.layers.forEach((layer, i) => {
      const x = xForV(layer.velocity);
      const y = yForD(layer.depth);
      if (i === 0) { ctx.moveTo(x, y); } else { ctx.lineTo(x, y); }
    });
    ctx.lineTo(xForV(this.layers[this.layers.length - 1].velocity), h - paddingB);
    ctx.lineTo(xForV(this.layers[0].velocity), h - paddingB);
    ctx.closePath();
    ctx.fillStyle = grad;
    ctx.fill();
    ctx.beginPath();
    this.layers.forEach((layer, i) => {
      const x = xForV(layer.velocity);
      const y = yForD(layer.depth);
      if (i === 0) { ctx.moveTo(x, y); } else { ctx.lineTo(x, y); }
    });
    ctx.strokeStyle = 'rgba(56, 189, 248, 0.95)';
    ctx.lineWidth = 2;
    ctx.stroke();
    let maxGradLayerIdx = -1;
    let maxGrad = 0;
    for (let i = 0; i < this.layers.length - 1; i++) {
      const d0 = this.layers[i].depth, d1 = this.layers[i + 1].depth;
      const v0 = this.layers[i].velocity, v1 = this.layers[i + 1].velocity;
      const g = Math.abs((v1 - v0) / Math.max(0.1, d1 - d0));
      if (g > maxGrad) { maxGrad = g; maxGradLayerIdx = i; }
    }
    if (maxGradLayerIdx >= 0 && maxGrad > 0.02) {
      const t = Math.max(0.3, maxGradLayerIdx / (this.layers.length - 1));
      const midD = (this.layers[maxGradLayerIdx].depth + this.layers[maxGradLayerIdx + 1].depth) / 2;
      const y = yForD(midD);
      ctx.strokeStyle = 'rgba(251, 146, 60, 0.6)';
      ctx.setLineDash([5, 3]);
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(paddingL, y);
      ctx.lineTo(w - paddingR, y);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = 'rgba(251, 146, 60, 0.9)';
      ctx.font = 'bold 10px -apple-system, sans-serif';
      ctx.textAlign = 'left';
      ctx.fillText(`温跃层 ${midD.toFixed(0)}m`, paddingL + 6, y - 4);
    }
    ctx.fillStyle = 'rgba(148, 163, 184, 0.8)';
    ctx.font = '10px -apple-system, sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText('声速 (m/s)', (paddingL + w - paddingR) / 2, h - 4);
    ctx.save();
    ctx.translate(10, (paddingT + h - paddingB) / 2);
    ctx.rotate(-Math.PI / 2);
    ctx.fillText('深度 (m)', 0, 0);
    ctx.restore();
  }
}
