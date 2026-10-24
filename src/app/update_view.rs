//! New versions: the check on start, the banner above the pages, Settings →
//! About, and the in-app update where the install can do it
//! (`crate::updater`: the Windows installer and zip, the Mac app).

use cosmic::iced::{Alignment, Length};
use cosmic::widget::{self, icon};
use cosmic::Element;

use super::{App, Message};
use crate::updater::{self, Plan, Progress, Staged};
use crate::updates::{self, Install, Release};

#[derive(Debug, Clone)]
pub enum UpdateMsg {
    SetCheck(bool),
    Check,
    Checked(Result<Option<Release>, String>),
    Dismiss,
    Now,
    Progress(Progress),
    Restart,
    DismissUpdated,
}

/// What the last check found.
#[derive(Default)]
pub enum UpdateStatus {
    #[default]
    NotChecked,
    Checking,
    UpToDate,
    Available(Release),
    Failed(String),
}

/// An in-app update the user started.
#[derive(Default)]
pub enum UpdateJob {
    #[default]
    Idle,
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    /// Downloaded and checked; installs when the app restarts.
    Ready(Staged),
    Failed(String),
}

#[derive(Default)]
pub struct Updates {
    pub status: UpdateStatus,
    pub job: UpdateJob,
    /// This is the first start of a new version (not a fresh install).
    pub just_updated: bool,
}

impl App {
    /// Notes the version that runs, and starts the check when it's wanted.
    pub(super) fn start_updates(&mut self) -> cosmic::app::Task<Message> {
        let s = &mut self.settings;
        // Settings saved by an older version (before `last_version`
        // existed, too) mean this is the first start after an update; a
        // fresh install has none.
        self.updates.just_updated = s.last_version.as_deref() != Some(updates::VERSION)
            && (s.last_version.is_some() || crate::settings::Settings::exists());
        if s.last_version.as_deref() != Some(updates::VERSION) {
            s.last_version = Some(updates::VERSION.into());
            s.save();
        }
        // Files an earlier update left behind.
        std::thread::spawn(updater::clean_up);
        if self.settings.check_for_updates && Install::detect().checks_github() {
            self.check_for_updates()
        } else {
            cosmic::app::Task::none()
        }
    }

    fn check_for_updates(&mut self) -> cosmic::app::Task<Message> {
        self.updates.status = UpdateStatus::Checking;
        cosmic::task::future(async { Message::Update(UpdateMsg::Checked(updates::check().await)) })
    }

    pub(super) fn update_updates(&mut self, msg: UpdateMsg) -> cosmic::app::Task<Message> {
        match msg {
            UpdateMsg::SetCheck(on) => {
                self.settings.check_for_updates = on;
                self.settings.save();
            }
            UpdateMsg::Check => return self.check_for_updates(),
            UpdateMsg::Checked(result) => {
                self.updates.status = match result {
                    Ok(Some(release)) => UpdateStatus::Available(release),
                    Ok(None) => UpdateStatus::UpToDate,
                    Err(e) => {
                        log::warn!("update check: {e}");
                        UpdateStatus::Failed(e)
                    }
                };
            }
            UpdateMsg::Dismiss => {
                if let UpdateStatus::Available(release) = &self.updates.status {
                    self.settings.dismissed_update = Some(release.version.clone());
                    self.settings.save();
                }
            }
            UpdateMsg::DismissUpdated => self.updates.just_updated = false,
            UpdateMsg::Now => {
                let plan = match &self.updates.status {
                    UpdateStatus::Available(release) => Plan::for_release(Install::detect(), release),
                    _ => None,
                };
                if let Some(plan) = plan {
                    if !matches!(self.updates.job, UpdateJob::Downloading { .. } | UpdateJob::Ready(_)) {
                        self.updates.job = UpdateJob::Downloading { done: 0, total: None };
                        return cosmic::task::stream(cosmic::iced::futures::StreamExt::map(updater::start(plan), |p| {
                            Message::Update(UpdateMsg::Progress(p))
                        }));
                    }
                }
            }
            UpdateMsg::Progress(progress) => {
                self.updates.job = match progress {
                    Progress::Downloading { done, total } => UpdateJob::Downloading { done, total },
                    Progress::Ready(staged) => UpdateJob::Ready(staged),
                    Progress::Failed(e) => UpdateJob::Failed(e),
                };
            }
            UpdateMsg::Restart => {
                if let UpdateJob::Ready(staged) = &self.updates.job {
                    // Quitting tells the server where playback stopped
                    // (App's Drop).
                    match updater::apply(staged) {
                        Ok(()) => return cosmic::iced::exit(),
                        Err(e) => {
                            log::warn!("update: {e}");
                            self.updates.job = UpdateJob::Failed(e);
                        }
                    }
                }
            }
        }
        cosmic::app::Task::none()
    }

    /// The "new version" and "updated" banners above the pages.
    pub(super) fn update_banners(&self) -> Vec<Element<'_, Message>> {
        let mut banners = Vec::new();
        if let UpdateStatus::Available(release) = &self.updates.status {
            // A started update stays up until it's done.
            let busy = !matches!(self.updates.job, UpdateJob::Idle);
            if busy || self.settings.dismissed_update.as_deref() != Some(release.version.as_str()) {
                let (msg, action) = self.update_message(release);
                let close = (!matches!(self.updates.job, UpdateJob::Downloading { .. }))
                    .then_some(Message::Update(UpdateMsg::Dismiss));
                banners.push(banner("software-update-available-symbolic", msg, action, close));
            }
        }
        if self.updates.just_updated {
            banners.push(banner(
                "emblem-ok-symbolic",
                format!("Nebula Player updated to {}.", updates::VERSION),
                widget::button::standard("What's new")
                    .on_press(Message::OpenUrl(format!("{}/tag/v{}", updates::RELEASES_URL, updates::VERSION)))
                    .into(),
                Some(Message::Update(UpdateMsg::DismissUpdated)),
            ));
        }
        banners
    }

    /// What the banner and Settings → About say about `release`, and the
    /// button beside it: Update now where this install can update itself,
    /// otherwise the release page.
    fn update_message(&self, release: &Release) -> (String, Element<'_, Message>) {
        let install = Install::detect();
        let in_app = Plan::for_release(install, release).is_some();
        let release_page = || -> Element<'_, Message> {
            widget::button::standard("Release page").on_press(Message::OpenUrl(release.url.clone())).into()
        };
        match &self.updates.job {
            UpdateJob::Idle if in_app => (
                format!("Nebula Player {} is available.", release.version),
                widget::button::suggested("Update now").on_press(Message::Update(UpdateMsg::Now)).into(),
            ),
            UpdateJob::Idle => (
                format!("Nebula Player {} is available. {}", release.version, install.how_to_update()),
                release_page(),
            ),
            UpdateJob::Downloading { done, total } => {
                let msg = match total {
                    Some(total) if *total > 0 => {
                        format!("Downloading Nebula Player {}… {}%", release.version, done * 100 / total)
                    }
                    _ => format!("Downloading Nebula Player {}… {:.1} MB", release.version, *done as f64 / 1e6),
                };
                let fraction = total.filter(|t| *t > 0).map_or(0.0, |t| *done as f32 / t as f32);
                let bar = cosmic::iced::widget::ProgressBar::new(0.0..=1.0, fraction).girth(4.0);
                (msg, widget::container(bar).width(Length::Fixed(140.0)).into())
            }
            UpdateJob::Ready(_) => (
                format!("Nebula Player {} is ready. Restart to finish updating.", release.version),
                widget::button::suggested("Restart now").on_press(Message::Update(UpdateMsg::Restart)).into(),
            ),
            UpdateJob::Failed(e) => (
                format!("Couldn't update: {e}"),
                widget::row::with_capacity(2)
                    .spacing(8)
                    .push(widget::button::standard("Try again").on_press(Message::Update(UpdateMsg::Now)))
                    .push(release_page())
                    .into(),
            ),
        }
    }

    /// Settings → About: the version, how it was installed, and updates.
    pub(super) fn view_about(&self) -> Element<'_, Message> {
        let install = Install::detect();
        let version = match updates::build_commit() {
            Some(commit) => format!("Version {} (build {commit})", updates::VERSION),
            None => format!("Version {}", updates::VERSION),
        };
        let mpv = match crate::mpv::api_version() {
            Some(v) => v.to_string(),
            None => crate::mpv::INSTALL_HINT.to_string(),
        };
        let section = widget::settings::section()
            .title("About")
            .add(
                widget::column::with_capacity(3)
                    .spacing(2)
                    .push(widget::text::title4("Nebula Player"))
                    .push(widget::text::body(version))
                    .push(widget::text::caption(format!("Made by Tim Singel  ·  {}  ·  {mpv}", install.label()))),
            )
            .add(
                widget::row::with_capacity(2)
                    .spacing(8)
                    .push(widget::button::standard("GitHub").on_press(Message::OpenUrl(updates::REPO_URL.into())))
                    .push(
                        widget::button::standard("Release notes")
                            .on_press(Message::OpenUrl(updates::RELEASES_URL.into())),
                    ),
            );

        if !install.checks_github() {
            return section.add(widget::text::caption(install.how_to_update())).into();
        }

        let status: Element<'_, Message> = match &self.updates.status {
            UpdateStatus::NotChecked => widget::text::body("Not checked yet").into(),
            UpdateStatus::Checking => widget::text::body("Checking…").into(),
            UpdateStatus::UpToDate => widget::text::body("You have the latest version").into(),
            UpdateStatus::Failed(e) => widget::text::body(format!("Couldn't check: {e}")).into(),
            UpdateStatus::Available(release) => {
                let (msg, action) = self.update_message(release);
                widget::column::with_capacity(4)
                    .spacing(6)
                    .push(widget::text::heading(format!("Version {} is available", release.version)))
                    .push_maybe((!matches!(self.updates.job, UpdateJob::Idle)).then(|| widget::text::body(msg)))
                    .push(action)
                    .push(widget::text::caption(install.how_to_update()))
                    .into()
            }
        };
        let checking = matches!(self.updates.status, UpdateStatus::Checking);
        section
            .add(
                widget::row::with_capacity(2)
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(widget::container(status).width(Length::Fill))
                    .push(
                        widget::button::standard("Check now")
                            .on_press_maybe((!checking).then_some(Message::Update(UpdateMsg::Check))),
                    ),
            )
            .add(widget::settings::item(
                "Check for updates on start",
                widget::toggler(self.settings.check_for_updates).on_toggle(|on| Message::Update(UpdateMsg::SetCheck(on))),
            ))
            .into()
    }
}

/// A notice above the pages: an icon, the text, a button, and maybe ✕.
fn banner<'a>(
    icon_name: &'static str,
    msg: String,
    action: Element<'a, Message>,
    close: Option<Message>,
) -> Element<'a, Message> {
    let mut row = widget::row::with_capacity(4)
        .push(icon::from_name(icon_name).size(16))
        .push(widget::text::body(msg).width(Length::Fill))
        .push(action)
        .spacing(12)
        .align_y(Alignment::Center);
    if let Some(close) = close {
        row = row.push(widget::button::icon(icon::from_name("window-close-symbolic")).on_press(close));
    }
    widget::container(row).padding([8, 16]).class(cosmic::theme::Container::Card).width(Length::Fill).into()
}
