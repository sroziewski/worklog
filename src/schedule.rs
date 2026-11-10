//! Registers the daily prompt with the OS scheduler: Task Scheduler on Windows, a systemd user
//! timer on Linux. Both catch up a start missed while the machine was off.

use std::path::Path;

use anyhow::Result;
use chrono::NaiveTime;

pub const TASK_NAME: &str = r"\Worklog\DailyPrompt";
pub const SYSTEMD_UNIT: &str = "worklog";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Task Scheduler definition: Monday-Friday at `at`, in the logged-on user's session (so the
/// window shows), started as soon as possible when the time was missed.
pub fn task_xml(exe: &Path, at: NaiveTime) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Suggests today's Jira worklogs from git activity.</Description></RegistrationInfo>
  <Triggers>
    <CalendarTrigger>
      <StartBoundary>2020-01-06T{time}</StartBoundary>
      <Enabled>true</Enabled>
      <ScheduleByWeek>
        <DaysOfWeek><Monday/><Tuesday/><Wednesday/><Thursday/><Friday/></DaysOfWeek>
        <WeeksInterval>1</WeeksInterval>
      </ScheduleByWeek>
    </CalendarTrigger>
  </Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <ExecutionTimeLimit>PT12H</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command></Exec></Actions>
</Task>
"#,
        time = at.format("%H:%M:%S"),
        exe = xml_escape(&exe.display().to_string()),
    )
}

/// (service, timer) unit files for a systemd user instance.
pub fn systemd_units(exe: &Path, at: NaiveTime) -> (String, String) {
    let service = format!(
        "[Unit]\nDescription=Suggest today's Jira worklogs\nAfter=graphical-session.target\nPartOf=graphical-session.target\n\n\
         [Service]\nType=oneshot\nExecStart=\"{}\"\n",
        // systemd expands %-specifiers and ends a quoted argument at '"'.
        exe.display().to_string().replace('%', "%%").replace('"', "\\\"")
    );
    let timer = format!(
        "[Unit]\nDescription=Daily Jira worklog prompt\n\n\
         [Timer]\nOnCalendar=Mon..Fri *-*-* {}\nPersistent=true\n\n\
         [Install]\nWantedBy=timers.target\n",
        at.format("%H:%M:00")
    );
    (service, timer)
}

#[cfg(windows)]
pub fn install(exe: &Path, at: NaiveTime) -> Result<String> {
    use anyhow::{Context, bail};
    let xml = task_xml(exe, at);
    // schtasks only reads task XML reliably as UTF-16 LE with a BOM.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let tmp = std::env::temp_dir().join("worklog-task.xml");
    std::fs::write(&tmp, bytes).context("writing task XML")?;
    let out = std::process::Command::new("schtasks").args(["/Create", "/F", "/TN", TASK_NAME, "/XML"]).arg(&tmp).output();
    let _ = std::fs::remove_file(&tmp);
    let out = out.context("running schtasks")?;
    if !out.status.success() {
        bail!("schtasks failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(format!("Task Scheduler task {TASK_NAME} runs {} Mon-Fri at {}", exe.display(), at.format("%H:%M")))
}

#[cfg(windows)]
pub fn uninstall() -> Result<String> {
    use anyhow::{Context, bail};
    let out = std::process::Command::new("schtasks").args(["/Delete", "/F", "/TN", TASK_NAME]).output().context("running schtasks")?;
    if !out.status.success() {
        bail!("schtasks failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(format!("removed {TASK_NAME}"))
}

#[cfg(not(windows))]
pub fn install(exe: &Path, at: NaiveTime) -> Result<String> {
    use anyhow::Context;
    let dir = dirs::config_dir().context("no config dir")?.join("systemd").join("user");
    std::fs::create_dir_all(&dir)?;
    let (service, timer) = systemd_units(exe, at);
    std::fs::write(dir.join(format!("{SYSTEMD_UNIT}.service")), service)?;
    std::fs::write(dir.join(format!("{SYSTEMD_UNIT}.timer")), timer)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", &format!("{SYSTEMD_UNIT}.timer")])?;
    Ok(format!("systemd user timer {SYSTEMD_UNIT}.timer runs {} Mon-Fri at {}", exe.display(), at.format("%H:%M")))
}

#[cfg(not(windows))]
pub fn uninstall() -> Result<String> {
    use anyhow::Context;
    let _ = systemctl(&["disable", "--now", &format!("{SYSTEMD_UNIT}.timer")]);
    let dir = dirs::config_dir().context("no config dir")?.join("systemd").join("user");
    for ext in ["service", "timer"] {
        let _ = std::fs::remove_file(dir.join(format!("{SYSTEMD_UNIT}.{ext}")));
    }
    systemctl(&["daemon-reload"])?;
    Ok(format!("removed {SYSTEMD_UNIT}.timer"))
}

#[cfg(not(windows))]
fn systemctl(args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("systemctl").arg("--user").args(args).output()?;
    anyhow::ensure!(out.status.success(), "systemctl --user {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    Ok(())
}
