//! AW-ELC lighting controller (USB 187c:0550) protocol.
//!
//! On the Aurora R16 (platform 0x0810) the HID descriptor only has unnumbered
//! 33-byte input and output reports, no feature reports. Commands therefore go
//! out as plain output reports (interrupt OUT endpoint) and each reply is
//! fetched with GET_REPORT(Input, ID 0). `crate::device` does that I/O per OS.
//!
//! Do not address numbered or feature reports on this controller: it answers
//! those with protocol errors and stays unresponsive until the PC is unplugged.

use std::thread::sleep;
use std::time::Duration;

use crate::device::Device;

pub const VENDOR_ID: u16 = 0x187C;
pub const PRODUCT_ID: u16 = 0x0550;
pub const REPORT_SIZE: usize = 33;
/// Report as passed to the OS: report ID byte (always 0) + report.
pub type RawReport = [u8; REPORT_SIZE + 1];

const ZONES_PER_PACKET: usize = REPORT_SIZE - 5;
const ACTIONS_PER_PACKET: usize = 3;

const CMD_REPORT: u8 = 0x20;
const CMD_USER_ANIMATION: u8 = 0x21;
const CMD_SELECT_ZONES: u8 = 0x23;
const CMD_ADD_ACTION: u8 = 0x24;
const CMD_DIM: u8 = 0x26;
const REPORT_CONFIG: u8 = 0x02;
const ANIM_NEW: u16 = 0x0001;
const ANIM_FINISH_PLAY: u16 = 0x0003;
// Volatile slot: plays immediately and is never written to controller flash.
const VOLATILE_SLOT: u16 = 0xFFFF;

// The controller is rate sensitive; OpenRGB pauses similarly between reports.
const REPLY_DELAY: Duration = Duration::from_millis(70);
const FINISH_PLAY_DELAY: Duration = Duration::from_secs(1);

pub const MODE_COLOR: u8 = 0x00;
pub const MODE_PULSE: u8 = 0x01;
pub const MODE_MORPH: u8 = 0x02;

pub type Rgb = [u8; 3];

/// One step of a zone's animation loop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Action {
    pub mode: u8,
    /// Milliseconds spent on this step.
    pub duration: u16,
    pub tempo: u16,
    pub color: Rgb,
}

impl Action {
    fn encode(&self) -> [u8; 8] {
        let [d0, d1] = self.duration.to_be_bytes();
        let [t0, t1] = self.tempo.to_be_bytes();
        let [r, g, b] = self.color;
        [self.mode, d0, d1, t0, t1, r, g, b]
    }
}

pub struct Controller {
    device: Device,
    pub platform: u16,
    pub zone_count: u8,
}

impl Controller {
    pub fn open() -> Result<Self, String> {
        let mut controller = Controller {
            device: Device::open()?,
            platform: 0,
            zone_count: 0,
        };

        let config = controller.transfer(&[0x03, CMD_REPORT, REPORT_CONFIG], REPLY_DELAY)?;
        controller.platform = u16::from_be_bytes([config[3], config[4]]);
        controller.zone_count = config[5];
        if controller.zone_count == 0 {
            return Err("controller reported zero lighting zones".into());
        }
        Ok(controller)
    }

    fn transfer(&mut self, payload: &[u8], delay: Duration) -> Result<[u8; REPORT_SIZE], String> {
        assert!(
            (3..=REPORT_SIZE).contains(&payload.len()),
            "invalid report length"
        );

        let mut output: RawReport = [0; REPORT_SIZE + 1];
        output[1..=payload.len()].copy_from_slice(payload);
        self.device.write_report(&output)?;

        sleep(delay);

        let mut input: RawReport = [0; REPORT_SIZE + 1];
        self.device.read_input_report(&mut input)?;

        let mut reply = [0u8; REPORT_SIZE];
        reply.copy_from_slice(&input[1..]);
        if reply[..2] != payload[..2] {
            return Err(format!(
                "unexpected reply to {:02x?}: {:02x?}",
                &payload[..3],
                &reply[..8]
            ));
        }
        Ok(reply)
    }

    fn user_animation(&mut self, subcommand: u16) -> Result<(), String> {
        let [s0, s1] = subcommand.to_be_bytes();
        let [a0, a1] = VOLATILE_SLOT.to_be_bytes();
        let delay = if subcommand == ANIM_FINISH_PLAY {
            FINISH_PLAY_DELAY
        } else {
            REPLY_DELAY
        };
        self.transfer(
            &[0x03, CMD_USER_ANIMATION, s0, s1, a0, a1, 0x00, 0x00],
            delay,
        )?;
        Ok(())
    }

    fn zones_command(&mut self, header: [u8; 3], zones: &[u8]) -> Result<(), String> {
        for chunk in zones.chunks(ZONES_PER_PACKET) {
            let mut payload = header.to_vec();
            payload.extend((chunk.len() as u16).to_be_bytes());
            payload.extend_from_slice(chunk);
            self.transfer(&payload, REPLY_DELAY)?;
        }
        Ok(())
    }

    /// Play one looping action sequence per phase. Phase `n` drives every
    /// `sequences.len()`-th zone starting at zone `n`, which gives the
    /// staggered "wave" look when the sequences are offset copies.
    pub fn play(&mut self, sequences: &[Vec<Action>], brightness: u8) -> Result<(), String> {
        assert!(
            !sequences.is_empty(),
            "animation needs at least one sequence"
        );
        let phases = sequences.len();

        self.user_animation(ANIM_NEW)?;
        for (phase, actions) in sequences.iter().enumerate() {
            let zones: Vec<u8> = (0..self.zone_count).skip(phase).step_by(phases).collect();
            for chunk in zones.chunks(ZONES_PER_PACKET) {
                self.zones_command([0x03, CMD_SELECT_ZONES, 0x01], chunk)?;
                for group in actions.chunks(ACTIONS_PER_PACKET) {
                    let mut payload = vec![0x03, CMD_ADD_ACTION];
                    for action in group {
                        payload.extend(action.encode());
                    }
                    self.transfer(&payload, REPLY_DELAY)?;
                }
            }
        }
        self.user_animation(ANIM_FINISH_PLAY)?;

        // The controller's dim value is inverted: 0 = full brightness.
        let all_zones: Vec<u8> = (0..self.zone_count).collect();
        self.zones_command([0x03, CMD_DIM, 100 - brightness.min(100)], &all_zones)
    }
}
