use tauri::{AppHandle, Emitter, Runtime};

use crate::lifecycle;

pub const MENU_OPEN: &str = "open-oga";
pub const MENU_QUIT: &str = "quit-oga";
pub const MENU_SETTINGS: &str = "open-settings";
pub const MENU_FIND_TASK: &str = "find-task";
pub const MENU_HELP: &str = "open-help";
pub const MENU_REPORT_PROBLEM: &str = "report-problem";
pub const MENU_TOGGLE_SIDEBAR: &str = "toggle-sidebar";
pub const MENU_TOGGLE_INSPECTOR: &str = "toggle-inspector";
pub const MENU_REFRESH: &str = "refresh-tasks";
pub const MENU_CLEAR_SELECTION: &str = "clear-selection";
pub const MENU_SHOW_ACTIVITY: &str = "show-activity";
pub const MENU_SHOW_REQUEST: &str = "show-request";
pub const MENU_SHOW_RESPONSE: &str = "show-response";
pub const MENU_ZOOM_IN: &str = "zoom-in";
pub const MENU_ZOOM_OUT: &str = "zoom-out";
pub const MENU_ZOOM_RESET: &str = "zoom-reset";
pub const MENU_HISTORY_BACK: &str = "history-back";
pub const MENU_HISTORY_FORWARD: &str = "history-forward";
pub const MENU_EVENT: &str = "oga-menu-command";

const HELP_URL: &str = "https://oga.desgn.space";
const REPORT_PROBLEM_URL: &str = "https://oga.desgn.space";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuCommand {
    ShowWindow,
    Quit,
    OpenHelp,
    ReportProblem,
    OpenSettings,
    FindTask,
    ToggleSidebar,
    ToggleInspector,
    RefreshTasks,
    ClearSelection,
    ShowActivity,
    ShowRequest,
    ShowResponse,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    HistoryBack,
    HistoryForward,
}

impl MenuCommand {
    fn from_id(id: &str) -> Option<Self> {
        match id {
            MENU_OPEN => Some(Self::ShowWindow),
            MENU_QUIT => Some(Self::Quit),
            MENU_HELP => Some(Self::OpenHelp),
            MENU_REPORT_PROBLEM => Some(Self::ReportProblem),
            MENU_SETTINGS => Some(Self::OpenSettings),
            MENU_FIND_TASK => Some(Self::FindTask),
            MENU_TOGGLE_SIDEBAR => Some(Self::ToggleSidebar),
            MENU_TOGGLE_INSPECTOR => Some(Self::ToggleInspector),
            MENU_REFRESH => Some(Self::RefreshTasks),
            MENU_CLEAR_SELECTION => Some(Self::ClearSelection),
            MENU_SHOW_ACTIVITY => Some(Self::ShowActivity),
            MENU_SHOW_REQUEST => Some(Self::ShowRequest),
            MENU_SHOW_RESPONSE => Some(Self::ShowResponse),
            MENU_ZOOM_IN => Some(Self::ZoomIn),
            MENU_ZOOM_OUT => Some(Self::ZoomOut),
            MENU_ZOOM_RESET => Some(Self::ZoomReset),
            MENU_HISTORY_BACK => Some(Self::HistoryBack),
            MENU_HISTORY_FORWARD => Some(Self::HistoryForward),
            _ => None,
        }
    }
}

pub fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};

    let settings = MenuItemBuilder::with_id(MENU_SETTINGS, "Settings...")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let app_submenu = SubmenuBuilder::new(app, "Oga")
        .about_with_text("About Oga", None)
        .separator()
        .item(&settings)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(
            &MenuItemBuilder::with_id(MENU_QUIT, "Quit Oga")
                .accelerator("CmdOrCtrl+Q")
                .build(app)?,
        )
        .build()?;

    let file_submenu = SubmenuBuilder::new(app, "File")
        .item(&accelerated_item(
            app,
            MENU_FIND_TASK,
            "Search Tasks",
            "CmdOrCtrl+K",
        )?)
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, "View")
        .item(&accelerated_item(
            app,
            MENU_ZOOM_IN,
            "Zoom In",
            "CmdOrCtrl+=",
        )?)
        .item(&accelerated_item(
            app,
            MENU_ZOOM_OUT,
            "Zoom Out",
            "CmdOrCtrl+-",
        )?)
        .item(&accelerated_item(
            app,
            MENU_ZOOM_RESET,
            "Actual Size",
            "CmdOrCtrl+0",
        )?)
        .separator()
        .fullscreen()
        .separator()
        .item(&accelerated_item(
            app,
            MENU_TOGGLE_SIDEBAR,
            "Toggle Sidebar",
            "CmdOrCtrl+/",
        )?)
        .item(
            &tauri::menu::MenuItemBuilder::with_id(MENU_TOGGLE_INSPECTOR, "Toggle Inspector")
                .accelerator("CmdOrCtrl+\\")
                .enabled(false)
                .build(app)?,
        )
        .separator()
        .item(&accelerated_item(
            app,
            MENU_REFRESH,
            "Refresh",
            "CmdOrCtrl+R",
        )?)
        .item(&item(app, MENU_CLEAR_SELECTION, "Clear Selection")?)
        .separator()
        .item(&accelerated_item(
            app,
            MENU_HISTORY_BACK,
            "Back",
            "CmdOrCtrl+[",
        )?)
        .item(&accelerated_item(
            app,
            MENU_HISTORY_FORWARD,
            "Forward",
            "CmdOrCtrl+]",
        )?)
        .separator()
        .item(&accelerated_item(
            app,
            MENU_SHOW_ACTIVITY,
            "Show Activity",
            "CmdOrCtrl+1",
        )?)
        .item(&accelerated_item(
            app,
            MENU_SHOW_REQUEST,
            "Show Request",
            "CmdOrCtrl+2",
        )?)
        .item(&accelerated_item(
            app,
            MENU_SHOW_RESPONSE,
            "Show Response",
            "CmdOrCtrl+3",
        )?)
        .build()?;

    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .item(&PredefinedMenuItem::undo(app, None)?)
        .item(&PredefinedMenuItem::redo(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, None)?)
        .item(&PredefinedMenuItem::copy(app, None)?)
        .item(&PredefinedMenuItem::paste(app, None)?)
        .separator()
        .item(&PredefinedMenuItem::select_all(app, None)?)
        .build()?;

    let window_submenu = SubmenuBuilder::new(app, "Window")
        .item(&PredefinedMenuItem::minimize(app, None)?)
        .item(&PredefinedMenuItem::maximize(app, Some("Zoom"))?)
        .separator()
        .item(&PredefinedMenuItem::close_window(app, None)?)
        .build()?;

    let help_submenu = SubmenuBuilder::new(app, "Help")
        .item(&item(app, MENU_HELP, "Oga Help")?)
        .item(&item(app, MENU_REPORT_PROBLEM, "Report a Problem")?)
        .build()?;

    MenuBuilder::new(app)
        .item(&app_submenu)
        .item(&file_submenu)
        .item(&edit_submenu)
        .item(&view_submenu)
        .item(&window_submenu)
        .item(&help_submenu)
        .build()
}

pub fn tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};

    let open = MenuItemBuilder::with_id(MENU_OPEN, "Open Oga")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "Quit Oga")
        .accelerator("CmdOrCtrl+Q")
        .build(app)?;
    MenuBuilder::new(app)
        .item(&open)
        .separator()
        .item(&quit)
        .build()
}

pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let Some(command) = MenuCommand::from_id(event.id().as_ref()) else {
        return;
    };
    match command {
        MenuCommand::ShowWindow => lifecycle::show_main_window(app),
        MenuCommand::Quit => lifecycle::quit(app),
        MenuCommand::OpenHelp => crate::open_url(app, HELP_URL),
        MenuCommand::ReportProblem => crate::open_url(app, REPORT_PROBLEM_URL),
        command => {
            let _ = app.emit(MENU_EVENT, command);
            lifecycle::show_main_window(app);
        }
    }
}

/// Lets the web view keep native menu items in step with what the current
/// route supports, e.g. graying out "Toggle Inspector" off the task screen.
#[tauri::command]
pub fn set_menu_item_enabled<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let Some(item) = menu.get(id.as_str()) else {
        return Ok(());
    };
    let Some(menu_item) = item.as_menuitem() else {
        return Ok(());
    };
    menu_item
        .set_enabled(enabled)
        .map_err(|error| error.to_string())
}

fn item<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    text: &str,
) -> tauri::Result<tauri::menu::MenuItem<R>> {
    tauri::menu::MenuItemBuilder::with_id(id, text).build(app)
}

fn accelerated_item<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    text: &str,
    accelerator: &str,
) -> tauri::Result<tauri::menu::MenuItem<R>> {
    tauri::menu::MenuItemBuilder::with_id(id, text)
        .accelerator(accelerator)
        .build(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_have_stable_commands() {
        assert_eq!(
            MenuCommand::from_id(MENU_SETTINGS),
            Some(MenuCommand::OpenSettings)
        );
        assert_eq!(MenuCommand::from_id(MENU_QUIT), Some(MenuCommand::Quit));
        assert_eq!(
            MenuCommand::from_id(MENU_FIND_TASK),
            Some(MenuCommand::FindTask)
        );
        assert_eq!(MenuCommand::from_id("unknown"), None);
    }
}
