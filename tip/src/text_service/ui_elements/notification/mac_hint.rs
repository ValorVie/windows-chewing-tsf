// SPDX-License-Identifier: GPL-3.0-or-later

use super::NotificationModel;
use crate::text_service::theme::{ThemeDetector, WindowsTheme};
use anyhow::Result;
use windows::Win32::{
    Graphics::Direct2D::{Common::*, *},
    UI::{
        HiDpi::SystemParametersInfoForDpi,
        WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS},
    },
};
use windows_core::HSTRING;
use windows_numerics::Vector2;

#[derive(Clone, Copy)]
pub(crate) struct Style {
    pub(super) padding: f32,
    pub(super) radius: f32,
    pub(super) shadow: f32,
}
impl Style {
    pub(crate) fn anchor(self, left: i32, bottom: i32, dpi: f32) -> (i32, i32) {
        let scale = dpi / 96.;
        // The transparent shadow margin is outside the caret-aligned bubble.
        (
            left - (self.shadow * scale).round() as i32,
            bottom + ((4. - self.shadow) * scale).round() as i32,
        )
    }
}
impl NotificationModel {
    pub(crate) fn suggestion(text: HSTRING) -> Self {
        let (family, size) = system_ui_font();
        model(
            text,
            matches!(ThemeDetector::get_app_theme(), Some(WindowsTheme::Dark)),
            size,
            family,
        )
    }
}
fn system_ui_font() -> (HSTRING, f32) {
    // Request 96-DPI metrics, then let the Direct2D target scale these DIPs.
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    if unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
            0,
            96,
        )
    }
    .is_ok()
    {
        let font = metrics.lfMessageFont;
        let length = font
            .lfFaceName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(font.lfFaceName.len());
        if length > 0 && font.lfHeight != 0 {
            return (
                HSTRING::from_wide(&font.lfFaceName[..length]),
                font.lfHeight.unsigned_abs() as f32,
            );
        }
    }
    (HSTRING::from("Segoe UI"), 12.)
}
fn model(text: HSTRING, dark: bool, system_font_size: f32, family: HSTRING) -> NotificationModel {
    let charcoal = D2D1_COLOR_F {
        r: 0.18,
        g: 0.18,
        b: 0.18,
        a: 1.,
    };
    let white = D2D1_COLOR_F {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 1.,
    };
    NotificationModel {
        text,
        font_family: family,
        font_size: (system_font_size + 1.).max(12.),
        fg_color: if dark { white } else { charcoal },
        bg_color: if dark { charcoal } else { white },
        hint_style: Some(Style {
            padding: (system_font_size / 2.).ceil(),
            radius: 7.,
            shadow: 10.,
        }),
        ..Default::default()
    }
}
pub(super) fn draw_bubble(
    dc: &ID2D1DeviceContext,
    style: Style,
    width: f32,
    height: f32,
    background: D2D1_COLOR_F,
) -> Result<()> {
    unsafe {
        dc.Clear(None);
        let size = D2D_SIZE_F {
            width: width + style.shadow * 2.,
            height: height + style.shadow * 2.,
        };
        let shadow = dc.CreateCompatibleRenderTarget(
            Some(&size),
            None,
            None,
            D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE,
        )?;
        shadow.BeginDraw();
        shadow.Clear(None);
        let brush = shadow.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 0.,
                g: 0.,
                b: 0.,
                a: 0.18,
            },
            None,
        )?;
        let bubble = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: style.shadow,
                top: style.shadow,
                right: style.shadow + width,
                bottom: style.shadow + height,
            },
            radiusX: style.radius,
            radiusY: style.radius,
        };
        let shadow_rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                top: bubble.rect.top + 1.,
                bottom: bubble.rect.bottom + 1.,
                ..bubble.rect
            },
            ..bubble
        };
        shadow.FillRoundedRectangle(&shadow_rect, &brush);
        shadow.EndDraw(None, None)?;
        let bitmap = shadow.GetBitmap()?;
        let blur = dc.CreateEffect(&CLSID_D2D1GaussianBlur)?;
        blur.SetInput(0, &bitmap, false);
        dc.DrawImage(
            &blur.GetOutput()?,
            Some(&Vector2 { X: 0., Y: 0. }),
            None,
            D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_COMPOSITE_MODE_SOURCE_OVER,
        );
        let brush = dc.CreateSolidColorBrush(&background, None)?;
        dc.FillRoundedRectangle(&bubble, &brush);
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
