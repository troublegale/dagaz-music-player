use std::time::{Duration, Instant};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

fn wait_for(
    session: &GlobalSystemMediaTransportControlsSession,
    expected: Status,
) -> windows::core::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if session.GetPlaybackInfo()?.PlaybackStatus()? == expected {
            return Ok(());
        }
        assert!(
            Instant::now() < deadline,
            "probe did not reach expected media state"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() -> windows::core::Result<()> {
    let sessions = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?
        .join()?
        .GetSessions()?;
    let mut owned = Vec::new();
    for index in 0..sessions.Size()? {
        let session = sessions.GetAt(index)?;
        if session.TryGetMediaPropertiesAsync()?.join()?.Title()? == "Dagaz Windows probe" {
            owned.push(session);
        }
    }
    assert_eq!(
        owned.len(),
        1,
        "open the Dagaz probe and start the network tone first"
    );
    let session = &owned[0];
    wait_for(session, Status::Playing)?;
    for _ in 0..2 {
        assert!(session.TryPauseAsync()?.join()?);
        wait_for(session, Status::Paused)?;
    }
    for _ in 0..2 {
        assert!(session.TryPlayAsync()?.join()?);
        wait_for(session, Status::Playing)?;
    }
    assert!(session.TryStopAsync()?.join()?);
    wait_for(session, Status::Stopped)?;
    println!("PASS: Dagaz native media pause/play/stop roundtrip");
    Ok(())
}
