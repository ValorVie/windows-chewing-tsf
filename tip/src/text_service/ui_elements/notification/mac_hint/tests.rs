// SPDX-License-Identifier: GPL-3.0-or-later

use super::super::{measure_model, paint_model};
use super::*;
use crate::ui::gfx::{create_render_target, d3d11_device};
use windows::Win32::{
    Foundation::GENERIC_WRITE,
    Graphics::{DirectWrite::*, Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM, Imaging::*},
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    },
};

fn sample(dark: bool) -> NotificationModel {
    model(
        HSTRING::from("⇥ 逝世／代錶"),
        dark,
        12.,
        HSTRING::from("Segoe UI"),
    )
}
fn color(value: &D2D1_COLOR_F) -> (f32, f32, f32, f32) {
    (value.r, value.g, value.b, value.a)
}
#[test]
fn d512_mac_style_dark_palette() {
    let m = sample(true);
    assert_eq!(color(&m.bg_color), (0.18, 0.18, 0.18, 1.));
    assert_eq!(color(&m.fg_color), (1., 1., 1., 1.));
}
#[test]
fn d512_mac_style_light_palette() {
    let m = sample(false);
    assert_eq!(color(&m.bg_color), (1., 1., 1., 1.));
    assert_eq!(color(&m.fg_color), (0.18, 0.18, 0.18, 1.));
}
#[test]
fn d512_mac_style_metrics_and_caret_gap() {
    let m = sample(true);
    assert_eq!(m.font_size, 13.);
    let style = m.hint_style.unwrap();
    assert_eq!((style.padding, style.radius, style.shadow), (6., 7., 10.));
    assert_eq!(style.anchor(100, 200, 96.), (90, 194));
    assert_eq!(style.anchor(100, 200, 192.), (80, 188));
    let small = model(HSTRING::from("⇥ 詞"), true, 9., HSTRING::from("Segoe UI"));
    assert_eq!(small.font_size, 12.);
    assert_eq!(small.hint_style.unwrap().padding, 5.);
}

struct Com;
impl Com {
    fn new() -> Self {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap() };
        Self
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
fn save_png(path: &std::path::Path, pixels: &[u8], width: u32, height: u32, dpi: f32) {
    unsafe {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
        let bitmap = wic
            .CreateBitmapFromMemory(
                width,
                height,
                &GUID_WICPixelFormat32bppPBGRA,
                width * 4,
                pixels,
            )
            .unwrap();
        let stream = wic.CreateStream().unwrap();
        stream
            .InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)
            .unwrap();
        let encoder = wic
            .CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())
            .unwrap();
        encoder
            .Initialize(&stream, WICBitmapEncoderNoCache)
            .unwrap();
        let mut frame = None;
        let mut options = None;
        encoder.CreateNewFrame(&mut frame, &mut options).unwrap();
        let frame = frame.unwrap();
        frame.Initialize(options.as_ref()).unwrap();
        frame.SetSize(width, height).unwrap();
        frame.SetResolution(dpi as f64, dpi as f64).unwrap();
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format).unwrap();
        assert_eq!(format, GUID_WICPixelFormat32bppBGRA);
        frame.WriteSource(&bitmap, std::ptr::null()).unwrap();
        frame.Commit().unwrap();
        encoder.Commit().unwrap();
    }
}
#[test]
fn d512_mac_style_native_pixels_and_png() {
    let _com = Com::new();
    unsafe {
        let factory: ID2D1Factory1 =
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
        let write: IDWriteFactory1 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let dc = create_render_target(&factory, &d3d11_device().unwrap()).unwrap();
        for dark in [false, true] {
            for dpi in [96., 192.] {
                let (family, base_size) = system_ui_font();
                let m = model(HSTRING::from("⇥ 逝世／代錶"), dark, base_size, family);
                let size = measure_model(&write, &m, dpi).unwrap();
                let width = size.hw_width as u32;
                let height = size.hw_height as u32;
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: dpi,
                    dpiY: dpi,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                    ..Default::default()
                };
                let bitmap = dc
                    .CreateBitmap(D2D_SIZE_U { width, height }, None, 0, &props)
                    .unwrap();
                dc.SetTarget(&bitmap);
                dc.SetDpi(dpi, dpi);
                dc.BeginDraw();
                dc.Clear(None);
                paint_model(&dc, &write, &m, size.width, size.height).unwrap();
                dc.EndDraw(None, None).unwrap();
                dc.SetTarget(None);
                let props = D2D1_BITMAP_PROPERTIES1 {
                    bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..props
                };
                let read = dc
                    .CreateBitmap(D2D_SIZE_U { width, height }, None, 0, &props)
                    .unwrap();
                read.CopyFromBitmap(None, &bitmap, None).unwrap();
                let map = read.Map(D2D1_MAP_OPTIONS_READ).unwrap();
                let mut pixels = Vec::with_capacity((width * height * 4) as usize);
                for y in 0..height {
                    pixels.extend_from_slice(std::slice::from_raw_parts(
                        map.bits.add((y * map.pitch) as usize),
                        (width * 4) as usize,
                    ));
                }
                read.Unmap().unwrap();
                if let Some(dir) = std::env::var_os("C2_HINT_PNG_DIR") {
                    let dir = std::path::PathBuf::from(dir);
                    assert!(dir.is_absolute());
                    std::fs::create_dir_all(&dir).unwrap();
                    save_png(
                        &dir.join(format!(
                            "hint-{}-{}.png",
                            if dark { "dark" } else { "light" },
                            dpi as u32
                        )),
                        &pixels,
                        width,
                        height,
                        dpi,
                    );
                }
                let pixel = |x: u32, y: u32| {
                    &pixels[((y * width + x) * 4) as usize..((y * width + x) * 4 + 4) as usize]
                };
                let scale = dpi / 96.;
                let at = |value: f32| (value * scale) as u32;
                let expected: u8 = if dark { 46 } else { 255 };
                assert_eq!(
                    pixel(at(13.), at(10. + size.height / 2.)),
                    &[expected, expected, expected, 255],
                    "body color at {dpi} dark={dark}"
                );
                assert!(pixel(at(10.), at(10.))[3] < 200, "rounded corner");
                let shadow = pixel(at(10. + size.width + 2.), at(10. + size.height / 2.))[3];
                assert!(shadow > 0 && shadow < 100, "soft shadow {shadow}");
                let text_pixels = pixels
                    .chunks_exact(4)
                    .filter(|p| p[3] == 255 && if dark { p[0] > 220 } else { p[0] < 100 })
                    .count();
                assert!(text_pixels > 10, "native DirectWrite text");
                assert_eq!(width, ((size.width + 20.) * scale).ceil() as u32);
                assert_eq!(height, ((size.height + 20.) * scale).ceil() as u32);
            }
        }
    }
}
#[test]
fn d512_mac_style_general_notification_keeps_existing_metrics() {
    let _com = Com::new();
    let write: IDWriteFactory1 =
        unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
    let mut m = sample(false);
    m.hint_style = None;
    assert!(NotificationModel::default().hint_style.is_none());
    let size = measure_model(&write, &m, 144.).unwrap();
    assert_eq!(size.hw_width, (size.width * 1.5 + 25.).ceil());
    assert_eq!(size.hw_height, (size.height * 1.5 + 25.).ceil());
}

#[test]
fn d512_mac_style_shadow_origin_keeps_caret_monitor() {
    // Adjacent monitors at 1920: a 10-DIP shadow shifts the HWND origin
    // across the seam while the caret remains on the right-hand monitor.
    let mut m = sample(true);
    m.hint_anchor = Some(windows::Win32::Foundation::POINT { x: 1925, y: 100 });
    let p = super::super::geometry_anchor(&m, windows::Win32::Foundation::POINT { x: 1915, y: 94 });
    assert_eq!((p.x, p.y), (1925, 100));
    let dpi_for = |x| if x < 1920 { 96. } else { 192. };
    assert_eq!(dpi_for(p.x), 192.);
}

#[test]
fn d512_mac_style_clamps_to_caret_work_area_at_monitor_seam() {
    use windows::Win32::Foundation::{POINT, RECT};
    let mut m = sample(true);
    m.hint_anchor = Some(POINT { x: 1925, y: 100 });
    let pos = super::super::clamp_hint_window(&m, 1915, 94, 240, 50, |p| {
        Some(if p.x < 1920 {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        } else {
            RECT {
                left: 1920,
                top: 0,
                right: 3840,
                bottom: 1080,
            }
        })
    });
    assert_eq!(pos, (1920, 94));
    m.hint_anchor = Some(POINT { x: -5, y: -10 });
    let pos = super::super::clamp_hint_window(&m, -15, -16, 240, 50, |_| {
        Some(RECT {
            left: -1920,
            top: -1080,
            right: 0,
            bottom: 0,
        })
    });
    assert_eq!(pos, (-240, -50));
}
