use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    Off,
    One,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "PlaybackWire")]
pub struct PlaybackState {
    #[schemars(range(min = 1))]
    pub epoch: i64,
    pub owner_device_id: Option<Uuid>,
    pub track_id: Option<Uuid>,
    pub queue: Vec<Uuid>,
    pub play_order: Vec<usize>,
    pub queue_index: Option<usize>,
    #[schemars(range(min = 0))]
    pub position_ms: i64,
    pub paused: bool,
    pub repeat: Repeat,
    pub shuffle_seed: Option<i64>,
    #[schemars(range(min = 0))]
    pub last_report_sequence: i64,
}
#[derive(Deserialize, schemars::JsonSchema)]
struct PlaybackWire {
    #[schemars(range(min = 1))]
    epoch: i64,
    owner_device_id: Option<Uuid>,
    track_id: Option<Uuid>,
    queue: Vec<Uuid>,
    play_order: Vec<usize>,
    queue_index: Option<usize>,
    #[schemars(range(min = 0))]
    position_ms: i64,
    paused: bool,
    repeat: Repeat,
    shuffle_seed: Option<i64>,
    #[schemars(range(min = 0))]
    last_report_sequence: i64,
}
impl TryFrom<PlaybackWire> for PlaybackState {
    type Error = &'static str;
    fn try_from(w: PlaybackWire) -> Result<Self, Self::Error> {
        if w.epoch <= 0 || w.position_ms < 0 || w.last_report_sequence < 0 {
            return Err("invalid epoch, position or report sequence");
        }
        if w.play_order.len() != w.queue.len() {
            return Err("play_order length differs from queue");
        }
        let mut seen = vec![false; w.queue.len()];
        for &index in &w.play_order {
            if index >= seen.len() || seen[index] {
                return Err("play_order must be a permutation");
            }
            seen[index] = true;
        }
        if w.shuffle_seed.is_none() && !w.play_order.iter().copied().eq(0..w.queue.len()) {
            return Err("unshuffled queue must use natural order");
        }
        match (w.queue_index, w.track_id) {
            (Some(index), Some(track)) if w.queue.get(index) == Some(&track) => {}
            (None, None) if w.queue.is_empty() && w.paused => {}
            _ => return Err("track and queue_index must identify the same occurrence"),
        }
        Ok(Self {
            epoch: w.epoch,
            owner_device_id: w.owner_device_id,
            track_id: w.track_id,
            queue: w.queue,
            play_order: w.play_order,
            queue_index: w.queue_index,
            position_ms: w.position_ms,
            paused: w.paused,
            repeat: w.repeat,
            shuffle_seed: w.shuffle_seed,
            last_report_sequence: w.last_report_sequence,
        })
    }
}
