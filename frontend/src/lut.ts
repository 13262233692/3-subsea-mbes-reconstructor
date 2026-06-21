import { ColorMode } from './types';

export interface RGB {
  r: number;
  g: number;
  b: number;
}

export interface LutStop {
  t: number;
  color: RGB;
}

export class ColorLUT {
  private stops: LutStop[];

  constructor(stops: LutStop[]) {
    this.stops = stops.slice().sort((a, b) => a.t - b.t);
  }

  static oceanDefault(): ColorLUT {
    return new ColorLUT([
      { t: 0.00, color: { r: 255, g: 255, b: 220 } },
      { t: 0.05, color: { r: 199, g: 233, b: 180 } },
      { t: 0.15, color: { r: 127, g: 205, b: 187 } },
      { t: 0.25, color: { r:  65, g: 182, b: 196 } },
      { t: 0.35, color: { r:  44, g: 162, b: 196 } },
      { t: 0.45, color: { r:  29, g: 140, b: 192 } },
      { t: 0.55, color: { r:  34, g: 113, b: 179 } },
      { t: 0.65, color: { r:  37, g:  87, b: 158 } },
      { t: 0.75, color: { r:  38, g:  64, b: 132 } },
      { t: 0.85, color: { r:  32, g:  44, b: 102 } },
      { t: 0.95, color: { r:  24, g:  28, b:  70 } },
      { t: 1.00, color: { r:  16, g:  14, b:  48 } },
    ]);
  }

  static seepHighlight(): ColorLUT {
    return new ColorLUT([
      { t: 0.00, color: { r: 255, g:  60, b:  60 } },
      { t: 0.15, color: { r: 255, g: 120, b:  50 } },
      { t: 0.30, color: { r: 255, g: 180, b:  40 } },
      { t: 0.50, color: { r: 255, g: 230, b: 120 } },
      { t: 0.70, color: { r: 180, g: 220, b: 180 } },
      { t: 0.85, color: { r:  80, g: 160, b: 200 } },
      { t: 1.00, color: { r:  30, g:  60, b: 150 } },
    ]);
  }

  static intensity(): ColorLUT {
    return new ColorLUT([
      { t: 0.00, color: { r:   0, g:   0, b:   0 } },
      { t: 0.15, color: { r:  30, g:  10, b:  60 } },
      { t: 0.30, color: { r:  90, g:  30, b: 120 } },
      { t: 0.45, color: { r: 160, g:  60, b: 100 } },
      { t: 0.60, color: { r: 220, g: 120, b:  60 } },
      { t: 0.75, color: { r: 250, g: 200, b:  80 } },
      { t: 0.90, color: { r: 255, g: 240, b: 180 } },
      { t: 1.00, color: { r: 255, g: 255, b: 255 } },
    ]);
  }

  sample(t: number): RGB {
    t = Math.max(0, Math.min(1, t));
    if (this.stops.length === 0) return { r: 0, g: 0, b: 0 };
    if (this.stops.length === 1) return { ...this.stops[0].color };
    if (t <= this.stops[0].t) return { ...this.stops[0].color };
    if (t >= this.stops[this.stops.length - 1].t) {
      return { ...this.stops[this.stops.length - 1].color };
    }
    for (let i = 0; i < this.stops.length - 1; i++) {
      const a = this.stops[i];
      const b = this.stops[i + 1];
      if (t >= a.t && t <= b.t) {
        const span = b.t - a.t;
        const alpha = span < 1e-9 ? 0 : (t - a.t) / span;
        return {
          r: Math.round(a.color.r + (b.color.r - a.color.r) * alpha),
          g: Math.round(a.color.g + (b.color.g - a.color.g) * alpha),
          b: Math.round(a.color.b + (b.color.b - a.color.b) * alpha),
        };
      }
    }
    return { ...this.stops[this.stops.length - 1].color };
  }

  toTextureData(size: number = 512): Uint8Array {
    const data = new Uint8Array(size * 3);
    for (let i = 0; i < size; i++) {
      const t = i / (size - 1);
      const c = this.sample(t);
      data[i * 3] = c.r;
      data[i * 3 + 1] = c.g;
      data[i * 3 + 2] = c.b;
    }
    return data;
  }

  toCssGradient(): string {
    const parts = this.stops.map(s => {
      const c = s.color;
      return `rgb(${c.r},${c.g},${c.b}) ${(s.t * 100).toFixed(1)}%`;
    });
    return `linear-gradient(to top, ${parts.join(', ')})`;
  }
}

export function getLUTForMode(mode: ColorMode): ColorLUT {
  switch (mode) {
    case 'depth': return ColorLUT.oceanDefault();
    case 'intensity': return ColorLUT.intensity();
    case 'seep': return ColorLUT.seepHighlight();
  }
}
