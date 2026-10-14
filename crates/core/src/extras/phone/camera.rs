#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//! The phone's camera as this computer's webcam.
//!
//! The phone sends JPEG pictures over its encrypted connection; here they
//! become a camera every app can pick ("OpenHop Camera").
//!
//! Linux: a v4l2loopback device (set up once, see [`setup_script`]).
//! Windows and macOS need a camera driver of their own (a Windows 11 virtual
//! camera, a macOS Camera Extension): not yet.

use std::path::PathBuf;

pub const LABEL: &str = "OpenHop Camera";

#[derive(Debug)]
pub enum CamError {
    /// No virtual camera on this computer yet.
    NeedSetup,
    Unsupported,
    Other(String),
}

/// The loopback device to write to: ours by name, or any loopback one.
#[cfg(target_os = "linux")]
pub fn find_device() -> Option<PathBuf> {
    let mut any = None;
    for e in std::fs::read_dir("/sys/class/video4linux").ok()?.flatten() {
        let name = std::fs::read_to_string(e.path().join("name")).unwrap_or_default();
        let dev = PathBuf::from("/dev").join(e.file_name());
        if name.trim() == LABEL {
            return Some(dev);
        }
        if name.contains("Dummy video device") || name.to_lowercase().contains("loopback") {
            any.get_or_insert(dev);
        }
    }
    any
}

#[cfg(not(target_os = "linux"))]
pub fn find_device() -> Option<PathBuf> {
    None
}

/// What sets the camera up once (run as administrator): loads v4l2loopback
/// now and at every start, installing it first if needed.
pub fn setup_script() -> String {
    format!(
        r#"set -e
if ! modinfo v4l2loopback >/dev/null 2>&1; then
  if command -v apt-get >/dev/null; then apt-get install -y v4l2loopback-dkms
  elif command -v dnf >/dev/null; then dnf install -y v4l2loopback
  elif command -v pacman >/dev/null; then pacman -S --noconfirm v4l2loopback-dkms
  elif command -v zypper >/dev/null; then zypper --non-interactive install v4l2loopback-kmp-default
  else echo "no package manager found" >&2; exit 2; fi
fi
echo v4l2loopback > /etc/modules-load.d/openhop-camera.conf
echo 'options v4l2loopback devices=1 video_nr=42 card_label="{LABEL}" exclusive_caps=1' > /etc/modprobe.d/openhop-camera.conf
modprobe -r v4l2loopback 2>/dev/null || true
modprobe v4l2loopback devices=1 video_nr=42 card_label="{LABEL}" exclusive_caps=1
"#
    )
}

/// Run the setup with the system's administrator prompt. Blocking.
#[cfg(target_os = "linux")]
pub fn setup() -> Result<(), String> {
    let out = std::process::Command::new("pkexec").args(["sh", "-c", &setup_script()]).output().map_err(|e| format!("couldn't ask for permission: {e}"))?;
    if out.status.success() && find_device().is_some() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().chars().take(200).collect::<String>())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn setup() -> Result<(), String> {
    Err("Not available on this system yet.".into())
}

/// Pictures in, camera out.
pub struct Camera {
    #[cfg(target_os = "linux")]
    dev: Option<(std::fs::File, u32, u32)>,
    pub frames: u64,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    pub fn new() -> Camera {
        Camera {
            #[cfg(target_os = "linux")]
            dev: None,
            frames: 0,
        }
    }

    /// Can this computer show the phone as a camera right now?
    pub fn ready() -> Result<(), CamError> {
        if !cfg!(target_os = "linux") {
            return Err(CamError::Unsupported);
        }
        find_device().map(|_| ()).ok_or(CamError::NeedSetup)
    }

    /// One picture from the phone.
    pub fn push_jpeg(&mut self, jpeg: &[u8]) -> Result<(), CamError> {
        let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).map_err(|e| CamError::Other(e.to_string()))?.to_rgb8();
        // Even sizes (two pixels share colour in YUYV).
        let (w, h) = (img.width() & !1, img.height() & !1);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let yuyv = to_yuyv(&img, w, h);
        self.write(&yuyv, w, h)?;
        self.frames += 1;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn write(&mut self, data: &[u8], w: u32, h: u32) -> Result<(), CamError> {
        use std::io::Write;
        if self.dev.as_ref().map(|d| (d.1, d.2) != (w, h)).unwrap_or(true) {
            self.dev = None;
            let path = find_device().ok_or(CamError::NeedSetup)?;
            let f = std::fs::OpenOptions::new().write(true).open(&path).map_err(|e| CamError::Other(format!("{}: {e}", path.display())))?;
            set_format(&f, w, h).map_err(CamError::Other)?;
            log::info!("phone camera on {} at {w}x{h}", path.display());
            self.dev = Some((f, w, h));
        }
        let (f, _, _) = self.dev.as_mut().unwrap();
        f.write_all(data).map_err(|e| {
            self.dev = None;
            CamError::Other(e.to_string())
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn write(&mut self, _: &[u8], _: u32, _: u32) -> Result<(), CamError> {
        Err(CamError::Unsupported)
    }
}

/// RGB to YUYV 4:2:2 (BT.601, studio range), what every camera app takes.
pub fn to_yuyv(img: &image::RgbImage, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 2) as usize);
    let yv = |r: i32, g: i32, b: i32| (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
    for y in 0..h {
        for x in (0..w).step_by(2) {
            let p0 = img.get_pixel(x, y).0;
            let p1 = img.get_pixel(x + 1, y).0;
            let (r0, g0, b0) = (p0[0] as i32, p0[1] as i32, p0[2] as i32);
            let (r1, g1, b1) = (p1[0] as i32, p1[1] as i32, p1[2] as i32);
            let (r, g, b) = ((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2);
            let u = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            let v = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            out.extend_from_slice(&[yv(r0, g0, b0), u, yv(r1, g1, b1), v]);
        }
    }
    out
}

/// VIDIOC_S_FMT: YUYV at w×h, for an output (writing) device.
#[cfg(target_os = "linux")]
fn set_format(f: &std::fs::File, w: u32, h: u32) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    const V4L2_BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
    const V4L2_PIX_FMT_YUYV: u32 = u32::from_le_bytes(*b"YUYV");
    const V4L2_FIELD_NONE: u32 = 1;
    const V4L2_COLORSPACE_SRGB: u32 = 8;
    // struct v4l2_format: type, then (aligned to 8) a 200-byte union whose
    // start is struct v4l2_pix_format.
    #[repr(C)]
    struct Pix {
        width: u32,
        height: u32,
        pixelformat: u32,
        field: u32,
        bytesperline: u32,
        sizeimage: u32,
        colorspace: u32,
        priv_: u32,
        flags: u32,
        ycbcr_enc: u32,
        quantization: u32,
        xfer_func: u32,
    }
    #[repr(C)]
    struct Format {
        type_: u32,
        _pad: u32,
        pix: Pix,
        _rest: [u8; 200 - std::mem::size_of::<Pix>()],
    }
    const _: () = assert!(std::mem::size_of::<Format>() == 208);
    let mut fmt = Format {
        type_: V4L2_BUF_TYPE_VIDEO_OUTPUT,
        _pad: 0,
        pix: Pix {
            width: w,
            height: h,
            pixelformat: V4L2_PIX_FMT_YUYV,
            field: V4L2_FIELD_NONE,
            bytesperline: w * 2,
            sizeimage: w * h * 2,
            colorspace: V4L2_COLORSPACE_SRGB,
            priv_: 0,
            flags: 0,
            ycbcr_enc: 0,
            quantization: 0,
            xfer_func: 0,
        },
        _rest: [0; 200 - std::mem::size_of::<Pix>()],
    };
    const VIDIOC_S_FMT: libc::c_ulong = 0xc0d0_5605;
    let r = unsafe { libc::ioctl(f.as_raw_fd(), VIDIOC_S_FMT as _, &mut fmt as *mut Format) };
    if r < 0 {
        return Err(format!("the camera device refused the picture size: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn colours() {
        let mut img = image::RgbImage::new(4, 2);
        for p in img.pixels_mut() {
            *p = image::Rgb([255, 255, 255]);
        }
        let y = super::to_yuyv(&img, 4, 2);
        assert_eq!(y.len(), 16);
        // White: Y 235, U and V 128.
        assert_eq!(&y[..4], &[235, 128, 235, 128]);
        assert!(super::setup_script().contains("card_label=\"OpenHop Camera\""));
    }
}
