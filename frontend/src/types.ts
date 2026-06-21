export interface Point3D {
  x: number;
  y: number;
  z: number;
  latitude?: number;
  longitude?: number;
  depth: number;
  intensity: number;
  reflectivity: number;
  beam_index?: number;
  ping_number: number;
  quality: number;
  artifact_flag?: number;
  seep_hint: number;
}

export interface PointBatchMsg {
  type: 'points';
  batch_id: number;
  timestamp: number;
  ping_start: number;
  ping_end: number;
  depth_min: number;
  depth_max: number;
  point_count: number;
  points_flat: number[];
}

export interface SvpUpdateMsg {
  type: 'svp';
  layer_count: number;
  depths: number[];
  velocities: number[];
}

export interface PipelineStatsMsg {
  type: 'stats';
  processed_pings: number;
  generated_points: number;
  buffer_flip_count: number;
  queued_points: number;
  timestamp: number;
}

export interface WelcomeMsg {
  type: 'welcome';
  server_version: string;
  supported_streams: string[];
}

export type ServerMessage =
  | PointBatchMsg
  | SvpUpdateMsg
  | PipelineStatsMsg
  | WelcomeMsg;

export interface SvpLayer {
  depth: number;
  velocity: number;
}

export type ColorMode = 'depth' | 'intensity' | 'seep';

export interface RenderStats {
  fps: number;
  renderedPoints: number;
  totalPoints: number;
}

export interface RenderConfig {
  pointSize: number;
  maxPoints: number;
  zScale: number;
  colorMode: ColorMode;
  showAxes: boolean;
  showGrid: boolean;
}
