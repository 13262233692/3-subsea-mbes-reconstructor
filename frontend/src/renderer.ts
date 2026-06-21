import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { PointCloudBuffer } from './ws_client';
import { RenderConfig, ColorMode } from './types';
import { ColorLUT, getLUTForMode, RGB } from './lut';

export interface RendererCallbacks {
  onFpsUpdate: (fps: number, rendered: number, total: number) => void;
}

export class PointCloudRenderer {
  private container: HTMLElement;
  private scene: THREE.Scene;
  private camera: THREE.PerspectiveCamera;
  private renderer: THREE.WebGLRenderer;
  private controls: OrbitControls;
  private pointCloud: THREE.Points | null = null;
  private geometry: THREE.BufferGeometry | null = null;
  private gridHelper: THREE.GridHelper | null = null;
  private axesHelper: THREE.AxesHelper | null = null;
  private config: RenderConfig;
  private lut: ColorLUT;
  private callbacks: RendererCallbacks;
  private lastFrameTime: number = 0;
  private frameCount: number = 0;
  private fpsTimer: number = 0;
  private running: boolean = false;
  private _animationId: number = 0;
  private depthRange: { min: number; max: number } = { min: 0, max: 1 };
  private dirtyBuffer: boolean = true;

  constructor(container: HTMLElement, config: RenderConfig, callbacks: RendererCallbacks) {
    this.container = container;
    this.config = { ...config };
    this.lut = getLUTForMode(this.config.colorMode);
    this.callbacks = callbacks;

    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(0x0a0e1a);
    this.scene.fog = new THREE.Fog(0x0a0e1a, 800, 6000);

    const rect = container.getBoundingClientRect();
    this.camera = new THREE.PerspectiveCamera(60, rect.width / rect.height, 0.1, 50000);
    this.camera.position.set(1200, 1400, 1800);

    this.renderer = new THREE.WebGLRenderer({
      antialias: true,
      alpha: false,
      powerPreference: 'high-performance',
      logarithmicDepthBuffer: false,
    });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.setSize(rect.width, rect.height);
    this.renderer.setClearColor(0x0a0e1a, 1);
    container.appendChild(this.renderer.domElement);

    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.07;
    this.controls.screenSpacePanning = true;
    this.controls.minDistance = 10;
    this.controls.maxDistance = 15000;
    this.controls.maxPolarAngle = Math.PI * 0.98;

    this._setupLights();
    this._initGeometry();
    this._initHelpers();
    this._onResize = this._onResize.bind(this);
    window.addEventListener('resize', this._onResize);
  }

  private _setupLights(): void {
    const amb = new THREE.AmbientLight(0xffffff, 0.85);
    this.scene.add(amb);
    const dir = new THREE.DirectionalLight(0xffffff, 0.5);
    dir.position.set(300, 800, 400);
    this.scene.add(dir);
    const rim = new THREE.DirectionalLight(0x66aaff, 0.25);
    rim.position.set(-500, 600, -300);
    this.scene.add(rim);
  }

  private _initGeometry(): void {
    this.geometry = new THREE.BufferGeometry();
    const cap = this.config.maxPoints;
    const pos = new Float32Array(cap * 3);
    const col = new Float32Array(cap * 3);
    const siz = new Float32Array(cap);
    this.geometry.setAttribute('position', new THREE.BufferAttribute(pos, 3).setUsage(THREE.DynamicDrawUsage));
    this.geometry.setAttribute('color', new THREE.BufferAttribute(col, 3).setUsage(THREE.DynamicDrawUsage));
    this.geometry.setAttribute('aPointSize', new THREE.BufferAttribute(siz, 1).setUsage(THREE.DynamicDrawUsage));
    this.geometry.setDrawRange(0, 0);

    const material = new THREE.ShaderMaterial({
      uniforms: {
        uPointSize: { value: this.config.pointSize },
        uPixelRatio: { value: this.renderer.getPixelRatio() },
        uSizeAttenuation: { value: 1.0 },
      },
      vertexShader: `
        attribute float aPointSize;
        uniform float uPointSize;
        uniform float uPixelRatio;
        uniform float uSizeAttenuation;
        varying vec3 vColor;
        varying float vIntensity;
        void main() {
          vColor = color;
          vIntensity = aPointSize;
          vec4 mv = modelViewMatrix * vec4(position, 1.0);
          float dist = -mv.z;
          float attn = mix(1.0, 300.0 / max(dist, 1.0), uSizeAttenuation);
          gl_PointSize = uPointSize * uPixelRatio * aPointSize * attn;
          gl_Position = projectionMatrix * mv;
        }
      `,
      fragmentShader: `
        varying vec3 vColor;
        varying float vIntensity;
        void main() {
          vec2 c = gl_PointCoord - vec2(0.5);
          float d2 = dot(c, c);
          if (d2 > 0.25) discard;
          float edge = smoothstep(0.25, 0.18, d2);
          float core = smoothstep(0.25, 0.0, d2);
          vec3 col = vColor * (0.7 + 0.5 * core);
          gl_FragColor = vec4(col, edge * (0.85 + 0.15 * vIntensity));
        }
      `,
      vertexColors: true,
      transparent: true,
      depthWrite: false,
      blending: THREE.NormalBlending,
    });

    this.pointCloud = new THREE.Points(this.geometry, material);
    this.pointCloud.frustumCulled = false;
    this.scene.add(this.pointCloud);
  }

  private _initHelpers(): void {
    this.gridHelper = new THREE.GridHelper(4000, 40, 0x1e3a5f, 0x0f1e33);
    (this.gridHelper.material as THREE.Material).transparent = true;
    (this.gridHelper.material as THREE.Material).opacity = 0.4;
    this.gridHelper.position.y = 0;
    this.scene.add(this.gridHelper);
    this.gridHelper.visible = this.config.showGrid;

    this.axesHelper = new THREE.AxesHelper(200);
    this.scene.add(this.axesHelper);
    this.axesHelper.visible = this.config.showAxes;
  }

  private _onResize(): void {
    const rect = this.container.getBoundingClientRect();
    if (rect.width < 2 || rect.height < 2) return;
    this.camera.aspect = rect.width / rect.height;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(rect.width, rect.height, false);
    const pr = Math.min(window.devicePixelRatio, 2);
    this.renderer.setPixelRatio(pr);
    if (this.pointCloud) {
      const m = this.pointCloud.material as THREE.ShaderMaterial;
      m.uniforms.uPixelRatio.value = pr;
    }
  }

  setPointSize(v: number): void {
    this.config.pointSize = v;
    if (this.pointCloud) {
      (this.pointCloud.material as THREE.ShaderMaterial).uniforms.uPointSize.value = v;
    }
  }

  setMaxPoints(v: number): void {
    this.config.maxPoints = v;
    if (this.geometry) {
      const cap = v;
      const pos = new Float32Array(cap * 3);
      const col = new Float32Array(cap * 3);
      const siz = new Float32Array(cap);
      this.geometry.setAttribute('position', new THREE.BufferAttribute(pos, 3).setUsage(THREE.DynamicDrawUsage));
      this.geometry.setAttribute('color', new THREE.BufferAttribute(col, 3).setUsage(THREE.DynamicDrawUsage));
      this.geometry.setAttribute('aPointSize', new THREE.BufferAttribute(siz, 1).setUsage(THREE.DynamicDrawUsage));
      this.geometry.setDrawRange(0, 0);
    }
    this.dirtyBuffer = true;
  }

  setZScale(v: number): void {
    this.config.zScale = v;
    this.dirtyBuffer = true;
  }

  setColorMode(mode: ColorMode): void {
    this.config.colorMode = mode;
    this.lut = getLUTForMode(mode);
    this.dirtyBuffer = true;
    this._updateColorbar();
  }

  setShowAxes(v: boolean): void {
    this.config.showAxes = v;
    if (this.axesHelper) this.axesHelper.visible = v;
  }

  setShowGrid(v: boolean): void {
    this.config.showGrid = v;
    if (this.gridHelper) this.gridHelper.visible = v;
  }

  setView(mode: 'top' | 'iso' | 'side'): void {
    const box = this._getBoundingBox();
    const cx = (box.min.x + box.max.x) / 2;
    const cy = (box.min.y + box.max.y) / 2;
    const cz = (box.min.z + box.max.z) / 2;
    const sx = box.max.x - box.min.x;
    const sy = box.max.y - box.min.y;
    const sz = box.max.z - box.min.z;
    const span = Math.max(sx, sy, sz, 200);
    this.controls.target.set(cx, cy, cz);
    switch (mode) {
      case 'top':
        this.camera.position.set(cx, cy + span * 2.0, cz + 0.001);
        break;
      case 'side':
        this.camera.position.set(cx + span * 1.6, cy + span * 0.4, cz);
        break;
      case 'iso':
      default:
        this.camera.position.set(cx + span * 1.2, cy + span * 1.4, cz + span * 1.4);
        break;
    }
    this.camera.lookAt(this.controls.target);
    this.controls.update();
  }

  private _getBoundingBox(): THREE.Box3 {
    const b = new THREE.Box3();
    if (this.geometry) {
      this.geometry.computeBoundingBox();
      if (this.geometry.boundingBox) {
        b.copy(this.geometry.boundingBox);
        return b;
      }
    }
    b.setFromCenterAndSize(new THREE.Vector3(0, 0, -600), new THREE.Vector3(2000, 2000, 2000));
    return b;
  }

  private _updateColorbar(): void {
    const el = document.getElementById('colorbar-gradient');
    if (!el) return;
    (el as HTMLElement).style.background = this.lut.toCssGradient();
  }

  updateFromBuffer(buf: PointCloudBuffer): void {
    this.dirtyBuffer = true;
    this._syncGeometry(buf);
  }

  forceSync(buf: PointCloudBuffer): void {
    this._syncGeometry(buf);
  }

  private _syncGeometry(buf: PointCloudBuffer): void {
    if (!this.geometry || !this.dirtyBuffer) return;
    const count = buf.filledCount;
    if (count === 0) {
      this.geometry.setDrawRange(0, 0);
      const posAttr = this.geometry.getAttribute('position') as THREE.BufferAttribute;
      const colAttr = this.geometry.getAttribute('color') as THREE.BufferAttribute;
      const sizeAttr = this.geometry.getAttribute('aPointSize') as THREE.BufferAttribute;
      posAttr.needsUpdate = true;
      colAttr.needsUpdate = true;
      sizeAttr.needsUpdate = true;
      this.dirtyBuffer = false;
      return;
    }
    const cap = (this.geometry.getAttribute('position') as THREE.BufferAttribute).count;
    const actual = Math.min(count, cap);
    const X = buf.X, Y = buf.Y, Z = buf.Z;
    const I = buf.I, R = buf.R, S = buf.S, Q = buf.Q;
    const zs = this.config.zScale;
    const mode = this.config.colorMode;
    let zMin = Infinity, zMax = -Infinity;
    const N = Math.min(actual, Math.min(X.length, Math.min(Y.length, Z.length)));
    for (let i = 0; i < N; i++) {
      const d = -Z[i] * zs;
      if (d < zMin) zMin = d;
      if (d > zMax) zMax = d;
    }
    if (isFinite(zMin) && isFinite(zMax) && zMax > zMin + 0.01) {
      this.depthRange.min = zMin;
      this.depthRange.max = zMax;
    }
    const dr = Math.max(0.001, this.depthRange.max - this.depthRange.min);
    const posArr = (this.geometry.getAttribute('position') as THREE.BufferAttribute).array as Float32Array;
    const colArr = (this.geometry.getAttribute('color') as THREE.BufferAttribute).array as Float32Array;
    const sizeArr = (this.geometry.getAttribute('aPointSize') as THREE.BufferAttribute).array as Float32Array;
    const lut = this.lut;
    for (let i = 0; i < N; i++) {
      const xi = i * 3;
      posArr[xi] = X[i];
      posArr[xi + 1] = Y[i];
      posArr[xi + 2] = -Z[i] * zs;
      let c: RGB;
      let szW = 1.0;
      switch (mode) {
        case 'depth': {
          const t = (posArr[xi + 2] - this.depthRange.min) / dr;
          c = lut.sample(t);
          break;
        }
        case 'intensity': {
          const t = Math.max(0, Math.min(1, I[i]));
          c = lut.sample(t);
          szW = 0.6 + 0.9 * t;
          break;
        }
        case 'seep': {
          const sh = S[i];
          if (sh > 0.1) {
            c = lut.sample(Math.max(0, 1 - sh * 1.2));
            szW = 1.3 + sh * 1.2;
          } else {
            const t = (posArr[xi + 2] - this.depthRange.min) / dr;
            const tint = Math.max(0, Math.min(1, I[i]));
            const dc = lut.sample(t);
            c = {
              r: Math.round(dc.r * (0.55 + 0.45 * tint)),
              g: Math.round(dc.g * (0.55 + 0.45 * tint)),
              b: Math.round(dc.b * (0.55 + 0.45 * tint)),
            };
            szW = 0.7 + 0.4 * tint;
          }
          break;
        }
      }
      colArr[xi] = c.r / 255;
      colArr[xi + 1] = c.g / 255;
      colArr[xi + 2] = c.b / 255;
      const qFactor = Q[i] >= 6 ? 1.1 : Q[i] >= 3 ? 0.9 : 0.6;
      sizeArr[i] = szW * qFactor;
    }
    this.geometry.setDrawRange(0, N);
    const posAttr = this.geometry.getAttribute('position') as THREE.BufferAttribute;
    const colAttr = this.geometry.getAttribute('color') as THREE.BufferAttribute;
    const sizeAttr = this.geometry.getAttribute('aPointSize') as THREE.BufferAttribute;
    posAttr.needsUpdate = true;
    colAttr.needsUpdate = true;
    sizeAttr.needsUpdate = true;
    this.dirtyBuffer = false;
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this._updateColorbar();
    this.lastFrameTime = performance.now();
    const loop = () => {
      if (!this.running) return;
      const now = performance.now();
      const dt = now - this.lastFrameTime;
      this.lastFrameTime = now;
      this.frameCount++;
      this.fpsTimer += dt;
      if (this.fpsTimer >= 500) {
        const fps = (this.frameCount * 1000) / this.fpsTimer;
        const rendered = this.geometry ? (this.geometry.drawRange.count as number) : 0;
        const total = this.config.maxPoints;
        this.callbacks.onFpsUpdate(fps, rendered, total);
        this.frameCount = 0;
        this.fpsTimer = 0;
      }
      this.controls.update();
      this.renderer.render(this.scene, this.camera);
      this._animationId = requestAnimationFrame(loop);
    };
    loop();
  }

  stop(): void {
    this.running = false;
    cancelAnimationFrame(this._animationId);
  }

  clearPoints(): void {
    if (this.geometry) {
      this.geometry.setDrawRange(0, 0);
      (this.geometry.getAttribute('position') as THREE.BufferAttribute).needsUpdate = true;
      (this.geometry.getAttribute('color') as THREE.BufferAttribute).needsUpdate = true;
    }
    this.depthRange = { min: 0, max: 1 };
    this.dirtyBuffer = true;
  }

  dispose(): void {
    this.stop();
    window.removeEventListener('resize', this._onResize);
    if (this.pointCloud) {
      this.scene.remove(this.pointCloud);
      this.geometry?.dispose();
      (this.pointCloud.material as THREE.Material).dispose();
    }
    this.gridHelper?.dispose();
    this.axesHelper?.dispose();
    this.renderer.dispose();
    if (this.renderer.domElement.parentElement === this.container) {
      this.container.removeChild(this.renderer.domElement);
    }
  }
}
