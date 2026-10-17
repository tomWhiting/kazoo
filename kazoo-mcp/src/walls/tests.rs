use std::path::Path;

use super::*;

/// No wall at all: a runtime directory with nothing listening in it.
fn nowhere() -> (tempfile::TempDir, Walls) {
    let dir = tempfile::Builder::new()
        .prefix("kw")
        .tempdir_in("/tmp")
        .unwrap();
    let (_, peer) = watch::channel(None);
    let walls = Walls::new(
        dir.path().join("kazoo-wall.sock"),
        Duration::from_secs(20),
        peer,
    );
    (dir, walls)
}

/// A joined wall standing in for one of `seat`'s, for `pick`.
fn joined(socket: &Path, seat: &str) -> Joined {
    let (feed, _) = mpsc::unbounded_channel();
    Joined {
        seat: seat.to_string(),
        wall: Arc::new(Wall::new(socket.to_path_buf(), seat.to_string())),
        feed,
        follower: tokio::spawn(async {}),
        notifier: tokio::spawn(async {}),
    }
}

#[tokio::test]
async fn nothing_is_joined_until_asked() {
    let (_dir, walls) = nowhere();
    assert_eq!(
        walls.on(None).await.unwrap_err(),
        "join a wall first: wall_walls lists them, and wall_join {as} joins one"
    );
    assert_eq!(
        walls.leave(None).await.unwrap_err(),
        "join a wall first: wall_walls lists them, and wall_join {as} joins one"
    );
}

#[tokio::test]
async fn a_wall_not_playing_is_found_and_said() {
    let (dir, walls) = nowhere();
    let found = walls.found().await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, HERE);
    assert_eq!(found[0].place, THIS_MAC);
    assert_eq!(
        found[0].playing,
        Err(format!(
            "not playing at {}",
            dir.path().join("kazoo-wall.sock").display()
        ))
    );
    assert!(found[0].seats.is_empty());
    assert_eq!(found[0].joined_as, None);
}

#[tokio::test]
async fn a_join_is_checked_kept_and_left() {
    let (_dir, walls) = nowhere();
    assert!(
        walls
            .join(None, "no/slash")
            .await
            .unwrap_err()
            .contains("is not a name the wall takes")
    );
    assert_eq!(
        walls.join(Some("elsewhere"), "Nova").await.unwrap_err(),
        "there is no wall 'elsewhere' to join; wall_walls lists them (here)"
    );
    // A wall not yet playing can be joined: its link waits for it.
    let _heard = walls.join_heard(None, "Nova").await.unwrap();
    assert_eq!(
        walls.join(None, "Other").await.unwrap_err(),
        "this seat is on here already, as Nova: wall_leave it first to come back as another name"
    );
    assert_eq!(walls.found().await[0].joined_as.as_deref(), Some("Nova"));
    assert!(walls.on(None).await.is_ok());
    assert!(walls.on(Some(HERE)).await.is_ok());
    assert_eq!(
        walls.on(Some("elsewhere")).await.unwrap_err(),
        "this seat is not on 'elsewhere'; it is on here"
    );
    assert_eq!(
        walls.leave(None).await.unwrap(),
        (HERE.to_string(), "Nova".to_string())
    );
    assert!(walls.on(None).await.is_err());
    // Back on, as another name.
    let _heard = walls.join_heard(None, "Nova Two").await.unwrap();
    assert_eq!(
        walls.found().await[0].joined_as.as_deref(),
        Some("Nova Two")
    );
}

#[tokio::test]
async fn with_more_than_one_wall_joined_the_tool_must_say_which() {
    let socket = Path::new("/nowhere/kazoo-wall.sock");
    let joined = BTreeMap::from([
        ("here".to_string(), joined(socket, "Nova")),
        ("upstairs".to_string(), joined(socket, "Nova")),
    ]);
    assert_eq!(
        Walls::pick(&joined, None).unwrap_err(),
        "this seat is on more than one wall: say which with wall (here, upstairs)"
    );
    assert_eq!(Walls::pick(&joined, Some("upstairs")).unwrap(), "upstairs");
    assert_eq!(
        Walls::pick(&joined, Some("cellar")).unwrap_err(),
        "this seat is not on 'cellar'; it is on here, upstairs"
    );
}

#[tokio::test]
async fn a_seat_tells_its_notifier_of_its_own_changes() {
    let (_dir, walls) = nowhere();
    let mut heard = walls.join_heard(None, "Nova").await.unwrap();
    walls.on(None).await.unwrap().own(7);
    assert!(matches!(heard.recv().await, Some(Feed::Own(7))));
}

#[tokio::test]
async fn leaving_stops_the_wall_s_event_line_and_notifier() {
    let (_dir, walls) = nowhere();
    walls.join(None, "Nova").await.unwrap();
    let joined = walls.joined.lock().await;
    let follower = joined[HERE].follower.abort_handle();
    let notifier = joined[HERE].notifier.abort_handle();
    drop(joined);
    assert!(!follower.is_finished());
    walls.leave(None).await.unwrap();
    for _ in 0..100 {
        if follower.is_finished() && notifier.is_finished() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the event line or the notifier kept going after the seat left");
}
