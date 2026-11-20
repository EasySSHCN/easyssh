//! R-20261119-006：窗口大小与位置记忆。
//! 存储 <cfg_dir>/window.json；启动时恢复，关闭时保存。全部 Result 静默忽略。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// tauri.conf.json 中的最小窗口尺寸（900x600）
const MIN_WIDTH: f64 = 900.0;
const MIN_HEIGHT: f64 = 600.0;

#[derive(Debug, Serialize, Deserialize)]
pub struct WindowGeom {
    pub x: i32,
    pub y: i32,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
    pub ver: u32,
}

fn geom_file() -> PathBuf {
    crate::configstate::default_base_dir().join("window.json")
}

fn load_geom() -> Option<WindowGeom> {
    let raw = std::fs::read_to_string(geom_file()).ok()?;
    let g: WindowGeom = serde_json::from_str(&raw).ok()?;
    // 字段合法性：ver 必须为 1，尺寸必须为有限正值
    if g.ver != 1 {
        return None;
    }
    if !g.width.is_finite() || !g.height.is_finite() || g.width <= 0.0 || g.height <= 0.0 {
        return None;
    }
    Some(g)
}

fn save_geom(g: &WindowGeom) {
    // 原子写：先写 .tmp 再 rename
    let path = geom_file();
    let tmp = path.with_extension("json.tmp");
    let data = match serde_json::to_string_pretty(g) {
        Ok(d) => d,
        Err(_) => return,
    };
    if std::fs::write(&tmp, data.as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 启动时恢复窗口几何：
/// - 记录矩形与任一显示器工作区有交集 → 恢复 size + position
/// - 完全无交集（如外接显示器已拔）→ 不恢复位置；仅当尺寸 >= 900x600 时恢复尺寸
/// - maximized=true → 恢复尺寸后最大化
/// 任何失败静默忽略，不影响启动。
pub fn apply_on_startup(window: &tauri::WebviewWindow) {
    if let Some(g) = load_geom() {
        // 枚举所有显示器工作区，检查矩形是否有交集
        let mut intersects = false;
        if let Ok(monitors) = window.available_monitors() {
            for m in monitors {
                let pos = m.position();
                let size = m.size();
                // 记录矩形与显示器矩形的 AABB 交集判定
                let g_x2 = g.x as f64 + g.width;
                let g_y2 = g.y as f64 + g.height;
                let m_x2 = (pos.x + size.width as i32) as f64;
                let m_y2 = (pos.y + size.height as i32) as f64;
                let overlap = (g.x as f64) < m_x2
                    && g_x2 > pos.x as f64
                    && (g.y as f64) < m_y2
                    && g_y2 > pos.y as f64;
                if overlap {
                    intersects = true;
                    break;
                }
            }
        }

        if intersects {
            let _ = window.set_size(tauri::LogicalSize::new(g.width, g.height));
            let _ = window.set_position(tauri::LogicalPosition::new(g.x, g.y));
        } else {
            // 无可见显示器：不恢复位置，仅恢复尺寸（不低于 min 尺寸时）
            if g.width >= MIN_WIDTH && g.height >= MIN_HEIGHT {
                let _ = window.set_size(tauri::LogicalSize::new(g.width, g.height));
            }
        }

        if g.maximized {
            let _ = window.maximize();
        }
    }
}

/// 关闭时保存窗口几何。窗口可能已销毁，全部静默容错。
/// 最大化时仍记录当前 outer_position 与 inner_size 作为还原底值。
pub fn save_on_close(window: &tauri::WebviewWindow) {
    let pos = match window.outer_position() {
        Ok(p) => p,
        Err(_) => return, // 窗口已销毁等
    };
    let size = match window.inner_size() {
        Ok(s) => s,
        Err(_) => return,
    };
    let maximized = window.is_maximized().unwrap_or(false);
    let g = WindowGeom {
        x: pos.x,
        y: pos.y,
        width: size.to_logical(window.scale_factor().unwrap_or(1.0)).width,
        height: size.to_logical(window.scale_factor().unwrap_or(1.0)).height,
        maximized,
        ver: 1,
    };
    save_geom(&g);
}
