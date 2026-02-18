use crate::doctor::DoctorReport;
use serde_json::Value;

pub fn render_peers_human(response: &Value) -> Option<String> {
    let peers = response.get("peers")?.as_array()?;
    if peers.is_empty() {
        return Some("No peers found.".to_string());
    }

    let mut rows: Vec<[String; 5]> = Vec::with_capacity(peers.len());
    for peer in peers {
        let agent_id = peer
            .get("agent_id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let addr = peer
            .get("addr")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let status = peer
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let rtt_ms = peer
            .get("rtt_ms")
            .and_then(Value::as_f64)
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "-".to_string());
        let source = peer
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        rows.push([agent_id, addr, status, rtt_ms, source]);
    }

    let mut widths = [8usize, 4usize, 6usize, 6usize, 6usize];
    for row in &rows {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(cell.len());
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {:>w3$}  {:<w4$}\n",
        "AGENT_ID",
        "ADDR",
        "STATUS",
        "RTT_MS",
        "SOURCE",
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
        w3 = widths[3],
        w4 = widths[4]
    ));

    out.push_str(&format!(
        "{}  {}  {}  {}  {}\n",
        "-".repeat(widths[0]),
        "-".repeat(widths[1]),
        "-".repeat(widths[2]),
        "-".repeat(widths[3]),
        "-".repeat(widths[4])
    ));

    for row in rows {
        out.push_str(&format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:>w3$}  {:<w4$}\n",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4]
        ));
    }

    Some(out.trim_end().to_string())
}

pub fn render_status_human(response: &Value) -> Option<String> {
    Some(format!(
        "Uptime: {}s\nPeers Connected: {}\nMessages Sent: {}\nMessages Received: {}",
        response.get("uptime_secs")?.as_u64()?,
        response.get("peers_connected")?.as_u64()?,
        response.get("messages_sent")?.as_u64()?,
        response.get("messages_received")?.as_u64()?
    ))
}

pub fn render_whoami_human(response: &Value) -> Option<String> {
    let name = response
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("(unset)");
    Some(format!(
        "Agent ID: {}\nPublic Key: {}\nName: {}\nVersion: {}\nUptime: {}s",
        response.get("agent_id")?.as_str()?,
        response.get("public_key")?.as_str()?,
        name,
        response.get("version")?.as_str()?,
        response.get("uptime_secs")?.as_u64()?
    ))
}

pub fn render_doctor_human(report: &DoctorReport) -> String {
    let marker = if report.ok { "✓" } else { "✗" };
    let mut out = format!(
        "Doctor: {marker} {}\nMode: {}\nState Root: {}",
        if report.ok { "PASS" } else { "FAIL" },
        report.mode,
        report.state_root
    );

    out.push_str("\n\nChecks:");
    for check in &report.checks {
        let check_marker = if check.ok { "✓" } else { "✗" };
        out.push_str(&format!(
            "\n  {check_marker} {}: {}",
            check.name, check.message
        ));
    }

    if !report.fixes_applied.is_empty() {
        out.push_str("\n\nFixes Applied:");
        for fix in &report.fixes_applied {
            out.push_str(&format!("\n  - {}: {}", fix.name, fix.message));
        }
    }

    out
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
