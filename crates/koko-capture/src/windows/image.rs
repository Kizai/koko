use super::{Image, frame::Pixels};
use crate::image_geometry::scaled_size;
use koko_proto::ImageFormat;
use std::{ffi::c_void, mem::ManuallyDrop, ptr::null};
use windows::{
    Win32::{
        Foundation::{E_FAIL, GENERIC_READ, RECT},
        Graphics::Imaging::*,
        System::{
            Com::{
                CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STATFLAG_NONAME, STATSTG,
                STREAM_SEEK_SET, StructuredStorage::PROPBAG2,
            },
            Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_R4},
        },
        UI::Shell::{DesktopWallpaper, IDesktopWallpaper, SHCreateMemStream},
    },
    core::{PCWSTR, PWSTR, Result, w},
};

pub(super) fn encode_pixels(
    pixels: &Pixels,
    edge: u16,
    quality: u8,
    format: ImageFormat,
) -> Result<Image> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_ALL)?;
        // Use 32bpp BGR to ignore the desktop surface's unused alpha channel.
        let bitmap = factory.CreateBitmapFromMemory(
            pixels.width,
            pixels.height,
            &GUID_WICPixelFormat32bppBGR,
            pixels.width * 4,
            &pixels.bytes,
        )?;
        encode(&factory, &bitmap, edge, quality, format)
    }
}

pub(super) fn wallpaper(primary: RECT) -> Result<Image> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_ALL)?;
        let desktop: IDesktopWallpaper = CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL)?;
        for index in 0..desktop.GetMonitorDevicePathCount()? {
            let device = ShellString(desktop.GetMonitorDevicePathAt(index)?);
            if desktop.GetMonitorRECT(device.as_const())? != primary {
                continue;
            }
            let file = ShellString(desktop.GetWallpaper(device.as_const())?);
            return if file.0.is_null() || *file.0.0 == 0 {
                let color = desktop.GetBackgroundColor()?.0;
                let rgb = color.to_le_bytes();
                let pixel = [rgb[2], rgb[1], rgb[0], 0];
                let bitmap = factory.CreateBitmapFromMemory(
                    1,
                    1,
                    &GUID_WICPixelFormat32bppBGR,
                    4,
                    &pixel,
                )?;
                encode(&factory, &bitmap, 1280, 80, ImageFormat::Jpeg)
            } else {
                let decoder = factory.CreateDecoderFromFilename(
                    file.as_const(),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )?;
                let bitmap = decoder.GetFrame(0)?;
                encode(&factory, &bitmap, 1280, 80, ImageFormat::Jpeg)
            };
        }
        Err(E_FAIL.into())
    }
}

struct ShellString(PWSTR);

impl ShellString {
    fn as_const(&self) -> PCWSTR {
        PCWSTR(self.0.0)
    }
}

impl Drop for ShellString {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.0.cast::<c_void>())) };
    }
}

fn encode(
    factory: &IWICImagingFactory,
    source: &IWICBitmapSource,
    edge: u16,
    quality: u8,
    format: ImageFormat,
) -> Result<Image> {
    unsafe {
        let (mut width, mut height) = (0, 0);
        source.GetSize(&mut width, &mut height)?;
        let size = scaled_size(width, height, edge);
        let scaler;
        let source = if size == (width, height) {
            source
        } else {
            scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(source, size.0, size.1, WICBitmapInterpolationModeFant)?;
            &scaler
        };
        let (width, height) = size;
        let (container, mut pixel_format) = match format {
            ImageFormat::Jpeg => (GUID_ContainerFormatJpeg, GUID_WICPixelFormat24bppBGR),
            ImageFormat::Png => (GUID_ContainerFormatPng, GUID_WICPixelFormat24bppBGR),
        };

        let stream = SHCreateMemStream(None).ok_or(E_FAIL)?;
        let encoder = factory.CreateEncoder(&container, null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let (mut frame, mut options) = (None, None);
        encoder.CreateNewFrame(&mut frame, &mut options)?;
        let frame = frame.ok_or(E_FAIL)?;
        if format == ImageFormat::Jpeg {
            let quality_property = PROPBAG2 {
                pstrName: PWSTR(w!("ImageQuality").0.cast_mut()),
                ..Default::default()
            };
            let quality = VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_R4,
                        Anonymous: VARIANT_0_0_0 {
                            fltVal: f32::from(quality) / 100.0,
                        },
                        ..Default::default()
                    }),
                },
            };
            options
                .as_ref()
                .ok_or(E_FAIL)?
                .Write(1, &quality_property, &quality)?;
        }
        frame.Initialize(options.as_ref())?;
        frame.SetSize(width, height)?;
        // WIC is allowed to negotiate a different output pixel format.
        frame.SetPixelFormat(&mut pixel_format)?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            source,
            &pixel_format,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        frame.WriteSource(&converter, null())?;
        frame.Commit()?;
        encoder.Commit()?;

        let mut stats = STATSTG::default();
        stream.Stat(&mut stats, STATFLAG_NONAME)?;
        let length = u32::try_from(stats.cbSize).map_err(|_| E_FAIL)?;
        let mut bytes = vec![0; length as usize];
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut read = 0;
        stream
            .Read(bytes.as_mut_ptr().cast(), length, Some(&mut read))
            .ok()?;
        if read != length {
            return Err(E_FAIL.into());
        }
        Ok(Image {
            bytes,
            width,
            height,
            format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wic_round_trips_scaled_png_and_jpeg() {
        let _apartment = super::super::Apartment::new().unwrap();
        // Synthetic uniform pixels exercise the WIC encoding and decoding path
        // without reading from an interactive desktop.
        let pixels = Pixels {
            bytes: [24, 80, 160, 0].repeat(64 * 32),
            width: 64,
            height: 32,
        };
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_ALL).unwrap();
            for (format, container, prefix) in [
                (ImageFormat::Png, GUID_ContainerFormatPng, &b"\x89PNG"[..]),
                (
                    ImageFormat::Jpeg,
                    GUID_ContainerFormatJpeg,
                    &b"\xff\xd8"[..],
                ),
            ] {
                let image = encode_pixels(&pixels, 32, 80, format).unwrap();
                assert_eq!((image.width, image.height, image.format), (32, 16, format));
                assert!(image.bytes.starts_with(prefix));

                let stream = SHCreateMemStream(Some(&image.bytes)).unwrap();
                let decoder = factory
                    .CreateDecoderFromStream(&stream, null(), WICDecodeMetadataCacheOnDemand)
                    .unwrap();
                assert_eq!(decoder.GetContainerFormat().unwrap(), container);
                let frame = decoder.GetFrame(0).unwrap();
                let (mut width, mut height) = (0, 0);
                frame.GetSize(&mut width, &mut height).unwrap();
                assert_eq!((width, height), (32, 16));
            }
        }
    }
}
