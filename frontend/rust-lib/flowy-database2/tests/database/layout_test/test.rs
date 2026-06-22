use std::time::Duration;

use crate::database::database_editor::DatabaseEditorTest;
use crate::database::layout_test::script::DatabaseLayoutTest;
use collab_database::database::gen_database_view_id;
use collab_database::views::DatabaseLayout;
use flowy_database2::entities::{CreateRowPayloadPB, RowsChangePB};
use flowy_database2::notification::DatabaseNotification;
use flowy_database2::services::setting::{BoardLayoutSetting, CalendarLayoutSetting};

#[tokio::test]
async fn board_layout_setting_test() {
  let mut test = DatabaseLayoutTest::new_board().await;
  let default_board_setting = BoardLayoutSetting::new();
  let new_board_setting = BoardLayoutSetting {
    hide_ungrouped_column: true,
    ..default_board_setting
  };

  // Assert the initial default board layout setting
  test
    .assert_board_layout_setting(default_board_setting)
    .await;

  // Update the board layout setting and assert the changes
  test
    .update_board_layout_setting(new_board_setting.clone())
    .await;
  test.assert_board_layout_setting(new_board_setting).await;
}

#[tokio::test]
async fn calendar_initial_layout_setting_test() {
  let test = DatabaseLayoutTest::new_calendar().await;
  let date_field = test.get_first_date_field().await;
  let default_calendar_setting = CalendarLayoutSetting::new(date_field.id.clone());

  // Assert the initial calendar layout setting
  test
    .assert_calendar_layout_setting(default_calendar_setting)
    .await;
}

#[tokio::test]
async fn calendar_get_events_test() {
  let test = DatabaseLayoutTest::new_calendar().await;

  // Assert the default calendar events
  test.assert_default_all_calendar_events().await;
}

#[tokio::test]
async fn grid_to_calendar_layout_test() {
  let mut test = DatabaseLayoutTest::new_no_date_grid().await;

  // Update layout to calendar and assert the number of calendar events
  test.update_database_layout(DatabaseLayout::Calendar).await;
  test.assert_all_calendar_events_count(3).await;
}

/// Regression test for https://github.com/AppFlowy-IO/AppFlowy/issues/8784
///
/// When a user creates a row from the grid view, every view in the database
/// (including the calendar) has the row written into its CRDT `row_orders`, so
/// the underlying data is always correct. The bug is purely about the Flutter
/// notification: `handle_did_update_row_orders` only sends the per-view
/// `DidUpdateRow` notification if the view's `DatabaseViewEditor` is present in
/// the cache.
///
/// Navigating away from the calendar back to the grid removes the calendar's
/// editor via `close_database_view`. With the old `get_view_editor` lookup the
/// calendar's notification was then silently dropped, so the calendar UI never
/// refreshed and the new row appeared to be missing. The fix uses
/// `get_or_init_view_editor`, which re-initializes the editor on demand so the
/// notification is still sent.
///
/// This test asserts on the notification (the actually-broken path), not on
/// `get_all_rows` (which reads from the CRDT and would pass even with the bug).
#[tokio::test]
async fn calendar_row_notification_sent_after_view_editor_closed_test() {
  // Set up a grid database (which includes a DateTime field needed for calendar).
  let test = DatabaseEditorTest::new_grid().await;
  let grid_view_id = test.view_id.clone();
  let database_id = test
    .sdk
    .database_manager
    .get_database_id_with_view_id(&grid_view_id)
    .await
    .unwrap();

  // Create a linked calendar view for the same database.
  let calendar_view_id = gen_database_view_id().to_string();
  test
    .sdk
    .database_manager
    .create_linked_view(
      "Calendar".to_string(),
      DatabaseLayout::Calendar,
      database_id,
      calendar_view_id.clone(),
      grid_view_id.clone(),
    )
    .await
    .unwrap();

  // Simulate the user opening then closing the calendar view, which removes its
  // editor from the cache — the exact condition that triggers the bug. Only the
  // grid view's editor should remain.
  let _ = test
    .editor
    .open_database_view(&calendar_view_id, None)
    .await;
  test
    .sdk
    .database_manager
    .close_database_view(&calendar_view_id)
    .await
    .unwrap();
  assert_eq!(test.editor.num_of_opening_views().await, 1);

  // Subscribe to the calendar view's row-update notification. We must NOT touch
  // `get_all_rows`/`open_database_view` on the calendar here — doing so would
  // re-initialize its editor and mask the bug.
  let mut rx = test
    .sdk
    .notification_sender
    .subscribe::<RowsChangePB>(&calendar_view_id, DatabaseNotification::DidUpdateRow);

  // Create a row from the grid view while the calendar editor is absent.
  let row_detail = test
    .editor
    .create_row(CreateRowPayloadPB {
      view_id: grid_view_id.clone(),
      ..Default::default()
    })
    .await
    .unwrap()
    .unwrap();

  // The calendar view must receive a DidUpdateRow notification reporting the
  // inserted row. The notification is dispatched from a spawned observer task,
  // so wait with a timeout.
  let change = tokio::time::timeout(Duration::from_secs(10), rx.recv())
    .await
    .unwrap_or_else(|_| {
      panic!(
        "calendar view never received a DidUpdateRow notification for row {} \
         created from the grid view",
        row_detail.row.id,
      )
    })
    .expect("notification channel closed unexpectedly");

  let created_row_id = row_detail.row.id.to_string();
  assert!(
    change
      .inserted_rows
      .iter()
      .any(|inserted| inserted.row_meta.id == created_row_id),
    "calendar's DidUpdateRow notification did not include the newly created row {}",
    created_row_id,
  );
}
