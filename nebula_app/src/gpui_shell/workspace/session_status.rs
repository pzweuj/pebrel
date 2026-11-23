//! Bottom session strip for the focused SSH pane: login identity and host resources.
//!
//! Samples stay on an exec channel of the already authenticated connection. The
//! terminal never sees the script, and a late sample cannot paint another pane.

use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Subscription, div, px,
};

use crate::gpui_shell::prelude::*;
use crate::i18n::Message;

use super::NebulaWorkspace;
use super::remote_files::remote_call;

const MARKER: &str = "__pebrel_status";
const PROBE_INTERVAL: Duration = Duration::from_secs(2);
const PROBE_BUDGET: Duration = Duration::from_secs(3);
const STATUS_SCRIPT: &str = r#"
echo __pebrel_status
user=$(id -un 2>/dev/null || true)
host=$(hostname 2>/dev/null || uname -n 2>/dev/null || true)
printf 'user=%s\nhost=%s\n' "$user" "$host"
if logins=$(LC_ALL=C who 2>/dev/null); then
  printf 'logins_begin=1\n'
  printf '%s\n' "$logins" | head -n 128 | sed 's/^/login=/'
fi
if [ -r /proc/stat ]; then
  awk 'NR==1 { idle=$5+$6; total=0; for (i=2; i<=NF; i++) total+=$i; printf "cpu_total=%d\ncpu_idle=%d\n", total, idle; exit }' /proc/stat
fi
if [ -r /proc/meminfo ]; then
  awk '
    $1=="MemTotal:" { total=$2 }
    $1=="MemAvailable:" { avail=$2 }
    $1=="MemFree:" { free=$2 }
    $1=="Buffers:" { buffers=$2 }
    $1=="Cached:" { cached=$2 }
    $1=="SwapTotal:" { swap=$2 }
    $1=="SwapFree:" { swap_free=$2 }
    END {
      if (avail == "") avail = free + buffers + cached
      printf "mem_total=%d\nmem_avail=%d\nswap_total=%d\nswap_free=%d\n", total+0, avail+0, swap+0, swap_free+0
    }
  ' /proc/meminfo
fi
if [ -r /proc/uptime ]; then
  awk '{ printf "uptime=%d\n", $1 }' /proc/uptime
fi
df -P / 2>/dev/null | awk 'NR==2 { printf "disk_total=%s\ndisk_used=%s\n", $2, $3 }'
if [ -r /proc/net/dev ]; then
  awk 'NR>2 {
    name=$1
    gsub(/:/, "", name)
    if (name == "lo") next
    rx += $2
    tx += $10
  } END { printf "net_rx=%d\nnet_tx=%d\n", rx+0, tx+0 }' /proc/net/dev
fi
"#;

#[derive(Clone, Copy, Default)]
struct Counters {
    at: Option<Instant>,
    cpu_total: u64,
    cpu_idle: u64,
    net_rx: u64,
    net_tx: u64,
}

#[derive(Default)]
struct Sample {
    logins: Option<Vec<LoginUser>>,
    user: Option<String>,
    host: Option<String>,
    cpu_total: Option<u64>,
    cpu_idle: Option<u64>,
    mem_total: Option<u64>,
    mem_avail: Option<u64>,
    swap_total: Option<u64>,
    swap_free: Option<u64>,
    disk_total: Option<u64>,
    disk_used: Option<u64>,
    net_rx: Option<u64>,
    net_tx: Option<u64>,
    uptime: Option<u64>,
}

#[derive(Default)]
pub(super) struct SessionStatus {
    logins: Option<Vec<LoginUser>>,
    visible: bool,
    pane: Option<u64>,
    destination: String,
    user: String,
    host: String,
    path: Option<String>,
    cpu: Option<u8>,
    memory: Option<u8>,
    swap: Option<u8>,
    disk: Option<u8>,
    down_bps: Option<u64>,
    up_bps: Option<u64>,
    uptime: Option<u64>,
    previous: Counters,
    probe: Option<gpui::Task<()>>,
    probing: bool,
    probe_at: Option<Instant>,
    generation: u64,
    copy_feedback: Option<Entity<crate::gpui_shell::copy_feedback::CopyFeedback>>,
    copy_feedback_sub: Option<Subscription>,
}

impl SessionStatus {
    fn ensure_copy_feedback<V: 'static>(&mut self, cx: &mut Context<V>) {
        if self.copy_feedback.is_none() {
            let feedback = cx.new(|_| crate::gpui_shell::copy_feedback::CopyFeedback::new());
            self.copy_feedback_sub = Some(cx.observe(&feedback, |_, _, cx| cx.notify()));
            self.copy_feedback = Some(feedback);
        }
    }

    fn clear_copy_feedback(&mut self, cx: &mut App) {
        if let Some(feedback) = &self.copy_feedback {
            feedback.update(cx, |feedback, cx| feedback.clear(cx));
        }
    }

    fn stop_probe(&mut self) {
        self.probe = None;
        self.probing = false;
        self.probe_at = None;
    }

    fn clear_reading(&mut self) {
        self.logins = None;
        self.cpu = None;
        self.memory = None;
        self.swap = None;
        self.disk = None;
        self.down_bps = None;
        self.up_bps = None;
        self.uptime = None;
        self.previous = Counters::default();
    }

    fn observe(
        &mut self,
        focus: Option<(u64, String, Option<String>)>,
        cx: &mut Context<NebulaWorkspace>,
    ) {
        let Some((pane, destination, path)) = focus else {
            if self.visible || self.probe.is_some() {
                self.visible = false;
                self.pane = None;
                self.destination.clear();
                self.user.clear();
                self.host.clear();
                self.path = None;
                self.generation = self.generation.wrapping_add(1);
                self.stop_probe();
                self.clear_reading();
                self.clear_copy_feedback(cx);
            }
            return;
        };
        let switched = self.pane != Some(pane) || self.destination != destination;
        let next_path = path.filter(|path| !path.is_empty());
        let path_changed = self.path != next_path;
        self.visible = true;
        self.pane = Some(pane);
        self.path = next_path;
        self.ensure_copy_feedback(cx);
        if switched || path_changed {
            self.clear_copy_feedback(cx);
        }
        if switched {
            self.destination = destination;
            let (user, host) = identity_from_destination(&self.destination);
            self.user = user;
            self.host = host;
            self.generation = self.generation.wrapping_add(1);
            self.stop_probe();
            self.clear_reading();
        }
        self.start_probe(cx);
    }

    fn start_probe(&mut self, cx: &mut Context<NebulaWorkspace>) {
        if self.probing {
            return;
        }
        let now = Instant::now();
        if self.probe_at.is_some_and(|at| now.saturating_duration_since(at) < PROBE_INTERVAL) {
            return;
        }
        let Some(pane) = self.pane else { return };
        let destination = self.destination.clone();
        let generation = self.generation;
        self.probing = true;
        self.probe_at = Some(now);
        self.probe = Some(cx.spawn(async move |this, cx| {
            let sampled = remote_call(move || async move {
                crate::ssh_session::exec_capture(
                    &destination,
                    "exec sh",
                    STATUS_SCRIPT.as_bytes(),
                    PROBE_BUDGET,
                )
                .await
            })
            .await;
            let _ = this.update(cx, |workspace, cx| {
                let status = &mut workspace.session_status;
                status.probing = false;
                if status.generation != generation || status.pane != Some(pane) {
                    return;
                }
                if let Some(Ok(text)) = sampled
                    && let Some(sample) = parse_status(&text)
                {
                    status.apply(sample, Instant::now());
                }
                cx.notify();
            });
        }));
    }

    fn apply(&mut self, sample: Sample, now: Instant) {
        self.logins = sample.logins;
        if let Some(user) = sample.user {
            self.user = user;
        }
        if let Some(host) = sample.host {
            self.host = host;
        }
        if let (Some(total), Some(avail)) = (sample.mem_total, sample.mem_avail) {
            self.memory = percent_used(total, avail);
        }
        if let (Some(total), Some(free)) = (sample.swap_total, sample.swap_free) {
            self.swap = percent_used(total, free);
        }
        if let (Some(total), Some(used)) = (sample.disk_total, sample.disk_used) {
            self.disk = percent_of(used, total);
        }
        if let Some(uptime) = sample.uptime {
            self.uptime = Some(uptime);
        }
        if let (Some(total), Some(idle), Some(rx), Some(tx)) =
            (sample.cpu_total, sample.cpu_idle, sample.net_rx, sample.net_tx)
        {
            if let Some(at) = self.previous.at {
                let elapsed = now.saturating_duration_since(at);
                if let Some(cpu) =
                    cpu_percent(self.previous.cpu_total, self.previous.cpu_idle, total, idle)
                {
                    self.cpu = Some(cpu);
                }
                if let Some(down) = byte_rate(self.previous.net_rx, rx, elapsed) {
                    self.down_bps = Some(down);
                }
                if let Some(up) = byte_rate(self.previous.net_tx, tx, elapsed) {
                    self.up_bps = Some(up);
                }
            }
            self.previous = Counters {
                at: Some(now),
                cpu_total: total,
                cpu_idle: idle,
                net_rx: rx,
                net_tx: tx,
            };
        }
    }
}

impl NebulaWorkspace {
    pub(super) fn observe_session_status(&mut self, cx: &mut Context<Self>) {
        self.session_status.observe(self.focused_remote_pane(cx), cx);
    }

    pub(super) fn attach_session_status(
        &self,
        body: gpui::AnyElement,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if !self.session_status.visible {
            return body;
        }
        v_flex()
            .size_full()
            .min_h_0()
            .child(div().flex_1().min_h_0().min_w_0().overflow_hidden().child(body))
            .child(self.session_status.render(cx))
            .into_any_element()
    }
}

impl SessionStatus {
    fn render(&self, cx: &App) -> gpui::AnyElement {
        let theme = cx.theme();
        let language = crate::gpui_shell::config::ui_language(cx);
        let muted = theme.muted_foreground;
        let identity = match (self.user.is_empty(), self.host.is_empty()) {
            (false, false) => format!("{}@{}", self.user, self.host),
            (false, true) => self.user.clone(),
            (true, false) => self.host.clone(),
            (true, true) => self.destination.clone(),
        };
        h_flex()
            .id("session-status")
            .debug_selector(|| "session-status".to_owned())
            .h(px(28.0))
            .px(px(12.0))
            .gap(px(10.0))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_xs()
            .text_color(theme.foreground)
            .child(div().flex_shrink_0().child(identity))
            .children(self.logins.as_ref().map(|users| {
                let label = language.text(Message::SessionStatusUsers);
                let names =
                    users.iter().map(|user| user.name.as_str()).collect::<Vec<_>>().join(", ");
                let detail = users
                    .iter()
                    .map(|user| format!("{}\n{}", user.name, user.sessions.join("\n")))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let tooltip =
                    format!("{}\n{}", language.text(Message::SessionStatusLoginTimes), detail);
                div()
                    .id("session-status-users")
                    .debug_selector(|| "session-status-users".to_owned())
                    .max_w(px(180.0))
                    .min_w_0()
                    .truncate()
                    .text_color(muted)
                    .tooltip(move |window, cx| {
                        let tooltip = tooltip.clone();
                        Tooltip::element(move |_, _| {
                            div()
                                .max_w(px(360.0))
                                .whitespace_normal()
                                .debug_selector(|| "session-status-users-tooltip".to_owned())
                                .child(tooltip.clone())
                        })
                        .build(window, cx)
                    })
                    .child(format!("{label}: {}", if names.is_empty() { "—" } else { &names }))
            }))
            .child(self.render_path(language, cx))
            .child(meter(
                "session-status-cpu",
                language.text(Message::SessionStatusCpu),
                self.cpu,
                self.probing,
                cx,
            ))
            .child(meter(
                "session-status-memory",
                language.text(Message::SessionStatusMemory),
                self.memory,
                self.probing,
                cx,
            ))
            .child(meter(
                "session-status-swap",
                language.text(Message::SessionStatusSwap),
                self.swap,
                self.probing,
                cx,
            ))
            .child(meter(
                "session-status-disk",
                language.text(Message::SessionStatusDisk),
                self.disk,
                self.probing,
                cx,
            ))
            .child(traffic(
                language.text(Message::SessionStatusDownload),
                language.text(Message::SessionStatusUpload),
                self.down_bps,
                self.up_bps,
                muted,
            ))
            .children(self.uptime.map(|seconds| {
                div().flex_shrink_0().text_color(muted).child(format_uptime(seconds))
            }))
            .into_any_element()
    }

    fn render_path(&self, language: crate::display::UiLanguage, cx: &App) -> gpui::AnyElement {
        let Some(path) = self.path.as_ref().filter(|path| !path.is_empty()) else {
            return div().flex_1().min_w_0().into_any_element();
        };
        let theme = cx.theme();
        let copied =
            self.copy_feedback.as_ref().is_some_and(|feedback| feedback.read(cx).is_copied());
        let copy_feedback = self.copy_feedback.clone();
        let path_for_copy = path.clone();
        let tooltip = if copied {
            language.text(Message::RemoteCopied).to_owned()
        } else {
            format!("{}\n{}", path, language.text(Message::RemoteCopyPath))
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .child(
                h_flex()
                    .id("session-status-path")
                    .debug_selector(|| "session-status-path".to_owned())
                    .max_w_full()
                    .min_w_0()
                    .h(px(22.0))
                    .px(px(6.0))
                    .gap(px(5.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_color(if copied { theme.foreground } else { theme.muted_foreground })
                    .hover(|el| el.bg(theme.list_hover).text_color(theme.foreground))
                    .active(|el| el.bg(theme.tab_active))
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(path_for_copy.clone()));
                        if let Some(feedback) = &copy_feedback {
                            feedback.update(cx, |feedback, cx| feedback.mark_copied(cx));
                        }
                    })
                    .child(div().min_w_0().truncate().child(path.clone()))
                    .child(if copied {
                        h_flex()
                            .flex_shrink_0()
                            .gap(px(3.0))
                            .text_color(theme.success)
                            .child(Icon::new(IconName::Check).size(px(12.0)))
                            .child(
                                div()
                                    .debug_selector(|| "session-status-path-copied".to_owned())
                                    .child(language.text(Message::RemoteCopied)),
                            )
                            .into_any_element()
                    } else {
                        Icon::new(IconName::Copy).size(px(12.0)).flex_shrink_0().into_any_element()
                    }),
            )
            .into_any_element()
    }
}

fn meter(
    id: &'static str,
    label: &'static str,
    value: Option<u8>,
    pending: bool,
    cx: &App,
) -> gpui::AnyElement {
    let theme = cx.theme();
    let text = match value {
        Some(percent) => format!("{percent}%"),
        None if pending => "…".to_owned(),
        None => "—".to_owned(),
    };
    let fill = match value {
        Some(percent) if percent >= 90 => theme.danger,
        Some(percent) if percent >= 70 => theme.warning,
        Some(_) => theme.primary,
        None => theme.border,
    };
    let width = value.map(|percent| gpui::relative(f32::from(percent) / 100.0));
    h_flex()
        .id(id)
        .flex_shrink_0()
        .gap(px(4.0))
        .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
        .child(div().text_color(theme.muted_foreground).child(label))
        .child(
            div()
                .w(px(36.0))
                .h(px(4.0))
                .rounded_full()
                .overflow_hidden()
                .bg(theme.border.opacity(0.7))
                .child(div().h_full().when_some(width, |bar, width| bar.w(width)).bg(fill)),
        )
        .child(div().w(px(32.0)).text_right().child(text))
        .into_any_element()
}

fn traffic(
    download: &'static str,
    upload: &'static str,
    down: Option<u64>,
    up: Option<u64>,
    muted: impl Into<gpui::Hsla> + Clone,
) -> gpui::AnyElement {
    let muted = muted.into();
    h_flex()
        .flex_shrink_0()
        .gap(px(6.0))
        .text_color(muted)
        .child(
            div()
                .id("session-status-download")
                .tooltip(move |window, cx| Tooltip::new(download).build(window, cx))
                .child(format!("↓ {}", down.map(format_rate).unwrap_or_else(|| "—".to_owned()))),
        )
        .child(
            div()
                .id("session-status-upload")
                .tooltip(move |window, cx| Tooltip::new(upload).build(window, cx))
                .child(format!("↑ {}", up.map(format_rate).unwrap_or_else(|| "—".to_owned()))),
        )
        .into_any_element()
}

fn identity_from_destination(destination: &str) -> (String, String) {
    let address = destination.trim().strip_prefix("ssh://").unwrap_or(destination.trim());
    let Some((user, host_port)) = address.rsplit_once('@') else {
        return (String::new(), address.to_owned());
    };
    let host = host_port.rsplit_once(':').map(|(host, _)| host).unwrap_or(host_port);
    (user.trim().to_owned(), host.trim().to_owned())
}

fn parse_status(text: &str) -> Option<Sample> {
    let mut lines = text.lines().map(str::trim);
    if !lines.any(|line| line == MARKER) {
        return None;
    }
    let mut sample = Sample::default();
    for line in lines {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        if value.chars().any(char::is_control) {
            continue;
        }
        match key {
            "user" if token(value) => sample.user = Some(value.to_owned()),
            "host" if token(value) => sample.host = Some(value.to_owned()),
            "logins_begin" => sample.logins = Some(Vec::new()),
            "login" => {
                if let Some(users) = sample.logins.as_mut() {
                    add_login(users, value);
                }
            },
            "cpu_total" => sample.cpu_total = value.parse().ok(),
            "cpu_idle" => sample.cpu_idle = value.parse().ok(),
            "mem_total" => sample.mem_total = value.parse().ok(),
            "mem_avail" => sample.mem_avail = value.parse().ok(),
            "swap_total" => sample.swap_total = value.parse().ok(),
            "swap_free" => sample.swap_free = value.parse().ok(),
            "disk_total" => sample.disk_total = value.parse().ok(),
            "disk_used" => sample.disk_used = value.parse().ok(),
            "net_rx" => sample.net_rx = value.parse().ok(),
            "net_tx" => sample.net_tx = value.parse().ok(),
            "uptime" => sample.uptime = value.parse().ok(),
            _ => {},
        }
    }
    Some(sample)
}

#[derive(Debug, PartialEq, Eq)]
struct LoginUser {
    name: String,
    sessions: Vec<String>,
}

fn add_login(users: &mut Vec<LoginUser>, line: &str) {
    let Some((name, session)) = line.split_once(char::is_whitespace) else { return };
    let session = session.trim();
    if !token(name) || name.len() > 256 || session.is_empty() || session.len() > 512 {
        return;
    }
    if let Some(user) = users.iter_mut().find(|user| user.name == name) {
        if user.sessions.len() < 16 && !user.sessions.iter().any(|existing| existing == session) {
            user.sessions.push(session.to_owned());
        }
    } else if users.len() < 64 {
        users.push(LoginUser { name: name.to_owned(), sessions: vec![session.to_owned()] });
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| !ch.is_whitespace() && !ch.is_control() && ch != '=')
}

fn percent_used(total: u64, available: u64) -> Option<u8> {
    percent_of(total.saturating_sub(available.min(total)), total)
}

fn percent_of(part: u64, total: u64) -> Option<u8> {
    if total == 0 {
        return Some(0);
    }
    Some(((part.min(total).saturating_mul(100) + total / 2) / total).min(100) as u8)
}

fn cpu_percent(previous_total: u64, previous_idle: u64, total: u64, idle: u64) -> Option<u8> {
    if total < previous_total || idle < previous_idle {
        return None;
    }
    let delta_total = total - previous_total;
    if delta_total == 0 {
        return None;
    }
    let delta_idle = (idle - previous_idle).min(delta_total);
    percent_of(delta_total - delta_idle, delta_total)
}

fn byte_rate(previous: u64, next: u64, elapsed: Duration) -> Option<u64> {
    if next < previous {
        return None;
    }
    let seconds = elapsed.as_secs_f64();
    if seconds < 0.2 {
        return None;
    }
    Some(((next - previous) as f64 / seconds).round() as u64)
}

fn format_rate(bytes_per_second: u64) -> String {
    const UNITS: [&str; 4] = ["B/s", "K/s", "M/s", "G/s"];
    if bytes_per_second < 1024 {
        return format!("{bytes_per_second} {}", UNITS[0]);
    }
    let mut value = bytes_per_second as f64 / 1024.0;
    for unit in &UNITS[1..] {
        if value < 1024.0 {
            return if value >= 10.0 {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{value:.0} {}", UNITS[3])
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "gpui-test-support")]
    #[gpui::test]
    fn online_user_strip_keeps_its_hover_target_with_long_account_names(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{AppContext as _, Render, Window};
        struct Probe(SessionStatus);
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(px(960.0))
                    .h(px(240.0))
                    .flex()
                    .flex_col()
                    .child(div().flex_1())
                    .child(self.0.render(cx))
            }
        }
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::gpui_shell::init(cx, Default::default());
            super::super::windowing::initialize(cx, crate::runtime_api::RuntimeHub::new());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let mut status = SessionStatus::default();
            status.apply(parse_status(
                    "__pebrel_status\nlogins_begin=1\nlogin=long_account_name pts/0 2026-11-14 09:00\nlogin=另一个账户 pts/1 2026-11-14 09:05\n",
                ).unwrap(), Instant::now());
            assert_eq!(status.logins.as_ref().unwrap().len(), 2);
            let probe = cx.new(|_| Probe(status));
            gpui_component::Root::new(probe, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let target = cx.debug_bounds("session-status-users").unwrap();
        assert!(target.size.width > px(0.0) && target.size.width <= px(180.0));
        assert_eq!(cx.debug_bounds("session-status").unwrap().size.height, px(28.0));
        cx.simulate_mouse_move(target.center(), None, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(600));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("session-status-users-tooltip").is_some());
        assert_eq!(cx.debug_bounds("session-status-users").unwrap(), target);
    }

    #[cfg(feature = "gpui-test-support")]
    #[gpui::test]
    fn clicking_bottom_path_copies_it_and_shows_timed_feedback(cx: &mut gpui::TestAppContext) {
        use crate::gpui_shell::copy_feedback::COPY_FEEDBACK_TTL;
        use gpui::{AppContext as _, Modifiers, MouseButton, Render, Window};

        struct Probe(SessionStatus);
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(px(960.0))
                    .h(px(240.0))
                    .flex()
                    .flex_col()
                    .child(div().flex_1())
                    .child(self.0.render(cx))
            }
        }
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::gpui_shell::init(cx, Default::default());
            super::super::windowing::initialize(cx, crate::runtime_api::RuntimeHub::new());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let probe = cx.new(|cx| {
                let mut status = SessionStatus::default();
                status.path = Some("/var/log/nginx".to_owned());
                status.ensure_copy_feedback(cx);
                Probe(status)
            });
            gpui_component::Root::new(probe, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let target = cx.debug_bounds("session-status-path").unwrap();
        assert!(target.size.width > px(0.0) && target.size.height > px(0.0));
        cx.simulate_mouse_down(target.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(target.center(), MouseButton::Left, Modifiers::default());
        // Move mouse away to confirm feedback remains visible outside hover.
        cx.simulate_mouse_move(gpui::point(px(4.0), px(4.0)), None, Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let copied = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        assert_eq!(copied.as_deref(), Some("/var/log/nginx"));
        assert!(cx.debug_bounds("session-status-path-copied").is_some());

        cx.executor().advance_clock(COPY_FEEDBACK_TTL);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("session-status-path-copied").is_none());
    }

    #[test]
    fn online_accounts_are_deduplicated_but_keep_each_login_time() {
        let sample = parse_status(
            "__pebrel_status\nlogins_begin=1\n\
            login=dev pts/0 2026-11-13 09:00 (10.0.0.1)\n\
            login=dev pts/1 2026-11-13 10:00 (10.0.0.2)\n\
            login=ops pts/2 2026-11-13 10:05\n",
        )
        .unwrap();
        let users = sample.logins.unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "dev");
        assert_eq!(users[0].sessions.len(), 2);
        assert!(users[0].sessions[1].contains("2026-11-13 10:00"));
        assert!(parse_status("__pebrel_status\n").unwrap().logins.is_none());
        assert!(
            parse_status("__pebrel_status\nlogins_begin=1\nlogin=\n")
                .unwrap()
                .logins
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn destination_identity_keeps_the_login_and_drops_the_port() {
        assert_eq!(
            identity_from_destination("ssh://dev@example.test:2222"),
            ("dev".to_owned(), "example.test".to_owned())
        );
        assert_eq!(identity_from_destination("host-only"), (String::new(), "host-only".to_owned()));
    }

    #[test]
    fn status_text_ignores_login_noise_and_computes_usage() {
        let banner = "Welcome to example\n__pebrel_status\nuser=root\nhost=box\n\
            cpu_total=1000\ncpu_idle=800\nmem_total=1000\nmem_avail=400\n\
            swap_total=0\nswap_free=0\ndisk_total=200\ndisk_used=50\n\
            net_rx=1000\nnet_tx=2000\nuptime=90061\n";
        let first = parse_status(banner).unwrap();
        assert_eq!(first.user.as_deref(), Some("root"));
        assert_eq!(percent_used(1000, 400), Some(60));
        assert_eq!(percent_used(0, 0), Some(0));
        assert_eq!(percent_of(50, 200), Some(25));
        assert!(parse_status("no marker\nuser=root\n").is_none());

        let mut status = SessionStatus::default();
        let started = Instant::now();
        status.apply(first, started);
        assert_eq!(status.memory, Some(60));
        assert_eq!(status.swap, Some(0));
        assert_eq!(status.disk, Some(25));
        assert_eq!(status.uptime, Some(90061));
        assert!(status.cpu.is_none(), "one sample has no cpu delta");

        let second = parse_status(
            "__pebrel_status\ncpu_total=1100\ncpu_idle=850\nnet_rx=3000\nnet_tx=2000\n",
        )
        .unwrap();
        status.apply(second, started + Duration::from_secs(2));
        assert_eq!(status.cpu, Some(50));
        assert_eq!(status.down_bps, Some(1000));
        assert_eq!(status.up_bps, Some(0));
        assert_eq!(format_uptime(90061), "1d 1h");
        assert_eq!(format_rate(1000), "1000 B/s");
        assert_eq!(format_rate(1536), "1.5 K/s");
    }

    #[test]
    fn wrapped_counters_do_not_invent_a_rate() {
        assert_eq!(cpu_percent(500, 100, 100, 50), None);
        assert_eq!(byte_rate(500, 100, Duration::from_secs(2)), None);
        assert_eq!(byte_rate(0, 100, Duration::from_millis(50)), None);
    }
}
