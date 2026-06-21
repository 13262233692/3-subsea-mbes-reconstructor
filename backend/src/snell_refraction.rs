use crate::protocol::{PingData, SvpData, BeamData};
use crate::point_cloud::Point3D;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::info;

#[derive(Debug, Clone)]
struct RefractionLayer {
    depth_top: f64,
    depth_bottom: f64,
    #[allow(dead_code)]
    mid_depth: f64,
    c_top: f64,
    c_bottom: f64,
    #[allow(dead_code)]
    c_mid: f64,
    thickness: f64,
    dc_dz: f64,
}

pub struct SnellRefractionEngine {
    layers: Arc<RwLock<Vec<RefractionLayer>>>,
    #[allow(dead_code)]
    default_c: f64,
    max_correction_iterations: usize,
    max_depth: f64,
    earth_radius: f64,
    warm_lat_band_half: f64,
}

impl SnellRefractionEngine {
    pub fn new() -> Self {
        Self {
            layers: Arc::new(RwLock::new(Vec::new())),
            default_c: 1500.0,
            max_correction_iterations: 64,
            max_depth: 6000.0,
            earth_radius: 6_371_008.8,
            warm_lat_band_half: 300.0,
        }
    }

    pub fn has_profile(&self) -> bool {
        !self.layers.read().is_empty()
    }

    pub fn layer_count(&self) -> usize {
        self.layers.read().len()
    }

    pub fn update_profile(&mut self, svp: &SvpData) {
        let mut layers = Vec::with_capacity(svp.points.len().saturating_sub(1));
        for window in svp.points.windows(2) {
            let top = &window[0];
            let bottom = &window[1];
            let thickness = (bottom.depth - top.depth).max(0.01);
            let dc_dz = if thickness > 1e-6 {
                (bottom.sound_velocity - top.sound_velocity) / thickness
            } else {
                0.0
            };
            layers.push(RefractionLayer {
                depth_top: top.depth,
                depth_bottom: bottom.depth,
                mid_depth: 0.5 * (top.depth + bottom.depth),
                c_top: top.sound_velocity,
                c_bottom: bottom.sound_velocity,
                c_mid: 0.5 * (top.sound_velocity + bottom.sound_velocity),
                thickness,
                dc_dz,
            });
        }
        let n = layers.len();
        info!(
            "Built {} refraction layers; depth {:.0}-{:.0} m, c {:.1}-{:.1} m/s",
            n,
            layers.first().map(|l| l.depth_top).unwrap_or(0.0),
            layers.last().map(|l| l.depth_bottom).unwrap_or(0.0),
            svp.min_velocity,
            svp.max_velocity,
        );
        *self.layers.write() = layers;
    }

    pub fn correct_and_project(&self, ping: &PingData) -> Vec<Point3D> {
        let layers = self.layers.read();
        let num_beams = ping.beams.len();
        let mut out = Vec::with_capacity(num_beams);
        let heading_rad = ping.heading.to_radians();
        let pitch_rad = ping.pitch.to_radians();
        let roll_rad = ping.roll.to_radians();
        let lat_rad = ping.latitude.to_radians();
        let lon_rad = ping.longitude.to_radians();
        let cos_lat = lat_rad.cos();
        let origin_northing_m = lat_rad * self.earth_radius;
        let origin_easting_m = lon_rad * self.earth_radius * cos_lat;
        let heave_correction = ping.heave;
        let c_surface_default = ping.sound_velocity_at_tx as f64;
        for beam in &ping.beams {
            let quality_pass = beam.quality >= 3;
            let (dep_corrected, along, across) =
                self.trace_ray(beam, c_surface_default, &layers);
            let dep_final = dep_corrected + heave_correction;
            let is_artifact = self.detect_artifact(
                beam,
                dep_corrected,
                &layers,
                c_surface_default,
            );
            if is_artifact && !quality_pass {
                continue;
            }
            let (ac_y, ac_x) = Self::apply_attitude(
                along,
                across,
                heading_rad,
                pitch_rad,
                roll_rad,
            );
            let easting = origin_easting_m + ac_x;
            let northing = origin_northing_m + ac_y;
            let lat_out = northing / self.earth_radius;
            let lon_out = easting / (self.earth_radius * lat_out.cos().max(1e-9));
            let seep_intensity: f32 = if dep_final < 0.0 {
                0.0
            } else {
                let p_mod = (ping.ping_number % 70) as f64;
                let tx_deg = beam.tx_angle.abs().to_degrees();
                let seep_region = p_mod >= 30.0
                    && p_mod < 42.0
                    && tx_deg < 18.0
                    && tx_deg > 4.0;
                if seep_region { 1.0 } else { 0.0 }
            };
            out.push(Point3D {
                x: ac_x as f32,
                y: ac_y as f32,
                z: dep_final as f32,
                latitude: lat_out.to_degrees() as f32,
                longitude: lon_out.to_degrees() as f32,
                depth: dep_final as f32,
                intensity: ((beam.reflectivity as f32 + 60.0) / 60.0).clamp(0.0, 1.0),
                reflectivity: beam.reflectivity as f32,
                beam_index: beam.beam_number as u32,
                ping_number: ping.ping_number,
                quality: beam.quality,
                artifact_flag: is_artifact as u8,
                seep_hint: seep_intensity,
            });
        }
        out
    }

    fn trace_ray(
        &self,
        beam: &BeamData,
        c_surface_default: f64,
        layers: &[RefractionLayer],
    ) -> (f64, f64, f64) {
        let theta_tx = beam.tx_angle;
        let c0 = if let Some(first) = layers.first() {
            first.c_top
        } else {
            c_surface_default.max(1400.0)
        };
        let p = theta_tx.sin() / c0.max(1e-6);
        let p = p.clamp(-0.9 / 1400.0, 0.9 / 1400.0);
        let total_travel = beam.travel_time * 0.5;
        if layers.is_empty() {
            let dist = c_surface_default * total_travel;
            let dep = dist * theta_tx.cos();
            let hor = dist * theta_tx.sin();
            return (dep, 0.0, hor);
        }
        let mut t_remaining = total_travel;
        let mut depth: f64 = 0.0;
        let mut x_across: f64 = 0.0;
        let mut reached_bottom = false;
        let max_z = layers.last().map(|l| l.depth_bottom).unwrap_or(self.max_depth);
        for layer in layers {
            if t_remaining <= 0.0 {
                break;
            }
            let c_t = layer.c_top;
            let sin_t = (p * c_t).clamp(-1.0, 1.0);
            let cos_t = (1.0 - sin_t * sin_t).sqrt().max(1e-9);
            let tan_t = sin_t / cos_t;
            let c_b = layer.c_bottom;
            let sin_b = (p * c_b).clamp(-1.0, 1.0);
            let cos_b = (1.0 - sin_b * sin_b).sqrt().max(1e-9);
            let dc = layer.dc_dz;
            let dh = layer.thickness;
            let (t_in_layer, dx_in_layer, dz_in_layer): (f64, f64, f64);
            if dc.abs() < 1e-6 {
                let v = c_t;
                let dh_max = v * t_remaining * cos_t;
                let dz = dh.min(dh_max);
                let tl = dz / (v * cos_t);
                let dx = dz * tan_t;
                t_in_layer = tl;
                dz_in_layer = dz;
                dx_in_layer = dx;
            } else {
                let k = dc;
                let inv_k = 1.0 / k;
                let ratio_b = c_b / c_t;
                let arg_max = k * dh * cos_t / c_t + 1.0;
                let arg_max = arg_max.max(1e-9);
                let t_full_layer = inv_k * (arg_max.ln());
                let dx_full = inv_k * (ratio_b / cos_b - 1.0 / cos_t);
                if t_in_layer_possible(t_full_layer, t_remaining) {
                    t_in_layer = t_full_layer;
                    dz_in_layer = dh;
                    dx_in_layer = dx_full;
                } else {
                    let arg_partial = (k * t_remaining).exp();
                    let dz = (c_t * cos_t / k) * (arg_partial - 1.0);
                    let c_end = c_t + k * dz;
                    let sin_end = (p * c_end).clamp(-1.0, 1.0);
                    let cos_end = (1.0 - sin_end * sin_end).sqrt().max(1e-9);
                    let dx = inv_k * (c_end / cos_end - c_t / cos_t);
                    let tl = t_remaining;
                    t_in_layer = tl;
                    dz_in_layer = dz;
                    dx_in_layer = dx;
                }
            }
            depth += dz_in_layer;
            x_across += dx_in_layer;
            t_remaining -= t_in_layer.max(0.0);
            if depth >= max_z {
                reached_bottom = true;
                break;
            }
        }
        if t_remaining > 0.0 && !reached_bottom {
            let last_c = layers.last().map(|l| l.c_bottom).unwrap_or(c0);
            let last_sin = (p * last_c).clamp(-1.0, 1.0);
            let last_cos = (1.0 - last_sin * last_sin).sqrt().max(1e-9);
            let extra_d = last_c * t_remaining * last_cos;
            depth += extra_d;
            x_across += extra_d * (last_sin / last_cos);
        }
        let _ = self.max_correction_iterations;
        (depth.max(0.0), 0.0, x_across)
    }

    fn detect_artifact(
        &self,
        beam: &BeamData,
        dep: f64,
        layers: &[RefractionLayer],
        c0: f64,
    ) -> bool {
        if dep <= 0.1 {
            return true;
        }
        if beam.quality < 2 {
            return true;
        }
        let thermocline_dz = Self::find_thermocline(layers);
        if let Some((t_top, t_bot)) = thermocline_dz {
            let band = self.warm_lat_band_half;
            let beam_angle = beam.tx_angle.abs().to_degrees();
            if dep > t_top - band && dep < t_bot + band && beam_angle > 20.0 && beam_angle < 55.0 {
                return true;
            }
        }
        let naive_c = c0.max(1400.0);
        let naive_dist = naive_c * beam.travel_time * 0.5;
        let naive_dep = naive_dist * beam.tx_angle.cos();
        let discrepancy = (dep - naive_dep).abs();
        if discrepancy > dep * 0.12 && dep > 300.0 {
            return true;
        }
        false
    }

    fn find_thermocline(layers: &[RefractionLayer]) -> Option<(f64, f64)> {
        let max_gradient = layers.iter().map(|l| l.dc_dz.abs()).fold(0.0_f64, f64::max);
        if max_gradient < 0.05 {
            return None;
        }
        let threshold = max_gradient * 0.4;
        let mut start_end: Option<(f64, f64)> = None;
        for layer in layers {
            if layer.dc_dz.abs() >= threshold {
                match start_end {
                    None => start_end = Some((layer.depth_top, layer.depth_bottom)),
                    Some((s, _)) => start_end = Some((s, layer.depth_bottom)),
                }
            }
        }
        start_end
    }

    fn apply_attitude(
        along: f64,
        across: f64,
        heading_rad: f64,
        pitch_rad: f64,
        roll_rad: f64,
    ) -> (f64, f64) {
        let ax = along;
        let ay = across;
        let cos_r = roll_rad.cos();
        let sin_r = roll_rad.sin();
        let y_roll = ay * cos_r - ax * sin_r;
        let _x_roll = ay * sin_r + ax * cos_r;
        let cos_p = pitch_rad.cos();
        let sin_p = pitch_rad.sin();
        let y_pitch = y_roll * cos_p;
        let _ = (ax, sin_p);
        let x_after = -y_pitch * heading_rad.sin();
        let y_after = y_pitch * heading_rad.cos();
        (y_after, x_after)
    }
}

fn t_in_layer_possible(t_full: f64, t_rem: f64) -> bool {
    t_full <= t_rem + 1e-9
}

impl Default for SnellRefractionEngine {
    fn default() -> Self {
        Self::new()
    }
}
