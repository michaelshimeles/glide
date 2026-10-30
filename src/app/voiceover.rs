//! Voice-over capture in the editor: `AVAudioRecorder` writing mono AAC
//! into the package while the preview plays (muted, so the speakers don't
//! bleed into the microphone).

use std::path::Path;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString, NSURL};

use crate::util::ns;

pub struct VoiceRecorder {
    recorder: Retained<AnyObject>,
    started: Instant,
    /// File name inside recording/.
    pub file: String,
    /// Source time where the narration starts.
    pub start_source: f64,
}

const K_AUDIO_FORMAT_MPEG4_AAC: u32 = u32::from_be_bytes(*b"aac ");

impl VoiceRecorder {
    /// Start recording the default microphone into `recording/<file>`.
    pub fn start(package: &Path, file: &str, start_source: f64) -> Result<Self> {
        use crate::capture::tcc::{self, Status};
        match tcc::mic_status() {
            Status::Denied | Status::Restricted => {
                bail!("Microphone access is off — enable DaisyCap in System Settings › Privacy & Security › Microphone")
            }
            _ => {}
        }
        let path = package.join("recording").join(file);
        let cls = AnyClass::get(c"AVAudioRecorder").ok_or_else(|| anyhow!("AVAudioRecorder unavailable"))?;
        unsafe {
            let keys: [&NSString; 4] = [&ns("AVFormatIDKey"), &ns("AVSampleRateKey"), &ns("AVNumberOfChannelsKey"), &ns("AVEncoderBitRateKey")];
            let values: [Retained<AnyObject>; 4] = [
                Retained::into_super(Retained::into_super(NSNumber::new_u32(K_AUDIO_FORMAT_MPEG4_AAC))).into(),
                Retained::into_super(Retained::into_super(NSNumber::new_f64(48_000.0))).into(),
                Retained::into_super(Retained::into_super(NSNumber::new_u32(1))).into(),
                Retained::into_super(Retained::into_super(NSNumber::new_u32(128_000))).into(),
            ];
            let value_refs: Vec<&AnyObject> = values.iter().map(|v| &**v).collect();
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &value_refs);
            let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            let mut err: *mut NSError = std::ptr::null_mut();
            let recorder: Option<Retained<AnyObject>> = msg_send![alloc, initWithURL: &*url, settings: &*settings, error: &mut err];
            let recorder = recorder.ok_or_else(|| {
                let msg = err.as_ref().map_or("unknown error".to_string(), |e| e.localizedDescription().to_string());
                anyhow!("couldn't start the microphone: {msg}")
            })?;
            let ready: Bool = msg_send![&*recorder, prepareToRecord];
            let started: Bool = msg_send![&*recorder, record];
            if !ready.as_bool() || !started.as_bool() {
                bail!("the microphone didn't start recording");
            }
            Ok(VoiceRecorder { recorder, started: Instant::now(), file: file.to_string(), start_source })
        }
    }

    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Stop and return the recorded length in ms.
    pub fn stop(self) -> f64 {
        let ms = self.started.elapsed().as_secs_f64() * 1000.0;
        unsafe {
            let _: () = msg_send![&*self.recorder, stop];
        }
        ms
    }
}
