#!/usr/bin/env python3
"""PR #3a: split timeout into wall-clock vs idle, add 10s heartbeat tick."""
import pathlib, sys

p = pathlib.Path("crates/bifrost-admin/src/remote_invoke/executor.rs")
s = p.read_text()
orig = s

# 1) Consts
old_const = "const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;"
assert old_const in s
new_const_block = (
    "const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;\n"
    "/// PR#3a: default idle timeout for shell.exec.\n"
    "const DEFAULT_IDLE_TIMEOUT_MS: u64 = 300_000;\n"
    "/// PR#3a: heartbeat tick interval (floor of idle-timeout granularity).\n"
    "const HEARTBEAT_INTERVAL_MS: u64 = 10_000;"
)
s = s.replace(old_const, new_const_block)

# 2) Add max_idle_ms to ResolvedShellPolicy (no serde; plain field)
old_resolved_field = "    max_timeout_ms: Option<u64>,\n    max_output_bytes: usize,"
assert old_resolved_field in s
s = s.replace(
    old_resolved_field,
    "    max_timeout_ms: Option<u64>,\n"
    "    /// PR#3a: idle timeout (None => DEFAULT_IDLE_TIMEOUT_MS)\n"
    "    max_idle_ms: Option<u64>,\n"
    "    max_output_bytes: usize,",
    1,
)

# 3) Add max_idle_ms to ShellPolicyMetadata (serde default struct-level covers it)
old_meta_field = "    max_timeout_ms: Option<u64>,\n    max_output_bytes: Option<usize>,"
assert old_meta_field in s
s = s.replace(
    old_meta_field,
    "    max_timeout_ms: Option<u64>,\n"
    "    max_idle_ms: Option<u64>,\n"
    "    max_output_bytes: Option<usize>,",
    1,
)

# 4) Wire max_idle_ms at policy resolve site
old_resolve = "            max_timeout_ms: policy_meta.max_timeout_ms.or(profile_meta.max_timeout_ms),"
assert old_resolve in s
s = s.replace(
    old_resolve,
    "            max_timeout_ms: policy_meta.max_timeout_ms.or(profile_meta.max_timeout_ms),\n"
    "            max_idle_ms: policy_meta.max_idle_ms.or(profile_meta.max_idle_ms),",
)

# 5) Rework timeout calc
old_timeout_calc = (
    "        let timeout_ms = command\n"
    "            .timeout_ms\n"
    "            .unwrap_or(policy.max_timeout_ms.unwrap_or(DEFAULT_SHELL_TIMEOUT_MS))\n"
    "            .min(policy.max_timeout_ms.unwrap_or(u64::MAX));"
)
assert old_timeout_calc in s
new_timeout_calc = (
    "        // PR#3a: split single wall-clock timeout into (wall_clock, idle).\n"
    "        let wall_clock_timeout_ms: Option<u64> = match (command.timeout_ms, policy.max_timeout_ms) {\n"
    "            (Some(c), Some(p)) => Some(c.min(p)),\n"
    "            (Some(c), None) => Some(c),\n"
    "            (None, Some(p)) => Some(p),\n"
    "            (None, None) => None,\n"
    "        };\n"
    "        let idle_timeout_ms: u64 = policy.max_idle_ms.unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);\n"
    "        let timeout_ms = wall_clock_timeout_ms.unwrap_or(u64::MAX);"
)
s = s.replace(old_timeout_calc, new_timeout_calc)

# 6) Replace timeout pin block with wall-clock + idle + heartbeat
old_timeout_pin = (
    "        let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));\n"
    "        tokio::pin!(timeout);"
)
assert old_timeout_pin in s
new_timeout_pin = (
    "        // PR#3a: wall-clock future (optional) + idle future + heartbeat tick.\n"
    "        let wall_clock = async {\n"
    "            match wall_clock_timeout_ms {\n"
    "                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,\n"
    "                None => std::future::pending::<()>().await,\n"
    "            }\n"
    "        };\n"
    "        tokio::pin!(wall_clock);\n"
    "        let mut idle_deadline = tokio::time::Instant::now()\n"
    "            + Duration::from_millis(idle_timeout_ms);\n"
    "        let idle_sleep = tokio::time::sleep_until(idle_deadline);\n"
    "        tokio::pin!(idle_sleep);\n"
    "        let mut heartbeat = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));\n"
    "        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);\n"
    "        let _ = heartbeat.tick().await; // swallow immediate tick"
)
s = s.replace(old_timeout_pin, new_timeout_pin)

# 7) Replace wall-clock arm with three arms
old_wall_arm = (
    "                _ = &mut timeout => {\n"
    "                    let _ = child.kill().await;\n"
    "                    let _ = child.wait().await;\n"
    "                    return Err(BifrostError::Network(format!(\n"
    "                        \"shell.exec timed out after {} ms (policy '{}')\",\n"
    "                        timeout_ms, policy.policy_id\n"
    "                    )));\n"
    "                }"
)
assert old_wall_arm in s
new_arms = (
    "                _ = &mut wall_clock => {\n"
    "                    let _ = child.kill().await;\n"
    "                    let _ = child.wait().await;\n"
    "                    return Err(BifrostError::Network(format!(\n"
    "                        \"shell.exec wall-clock timeout after {} ms (policy '{}')\",\n"
    "                        timeout_ms, policy.policy_id\n"
    "                    )));\n"
    "                }\n"
    "                _ = &mut idle_sleep => {\n"
    "                    let _ = child.kill().await;\n"
    "                    let _ = child.wait().await;\n"
    "                    return Err(BifrostError::Network(format!(\n"
    "                        \"shell.exec idle timeout after {} ms of no output (policy '{}')\",\n"
    "                        idle_timeout_ms, policy.policy_id\n"
    "                    )));\n"
    "                }\n"
    "                _ = heartbeat.tick() => {\n"
    "                    // PR#3a: hook for PR#3b StreamFrame::Heartbeat. Does\n"
    "                    // NOT reset idle_deadline — liveness must come from\n"
    "                    // actual process output, not from our own tick.\n"
    "                }"
)
s = s.replace(old_wall_arm, new_arms)

# 8) Reset idle deadline on stdout read
old_stdout_reset = (
    "                    } else {\n"
    "                        // PR#2: hash + total BEFORE the cap so we retain truth even on overflow\n"
    "                        stdout_total_bytes += read as u64;\n"
    "                        stdout_hasher.update(&stdout_buf[..read]);"
)
assert old_stdout_reset in s
s = s.replace(
    old_stdout_reset,
    "                    } else {\n"
    "                        // PR#3a: real stdout read resets the idle deadline.\n"
    "                        idle_deadline = tokio::time::Instant::now()\n"
    "                            + Duration::from_millis(idle_timeout_ms);\n"
    "                        idle_sleep.as_mut().reset(idle_deadline);\n"
    "                        // PR#2: hash + total BEFORE the cap\n"
    "                        stdout_total_bytes += read as u64;\n"
    "                        stdout_hasher.update(&stdout_buf[..read]);",
)

# 9) Reset idle deadline on stderr read
old_stderr_reset = (
    "                    } else {\n"
    "                        // PR#2: hash + total BEFORE the cap\n"
    "                        stderr_total_bytes += read as u64;\n"
    "                        stderr_hasher.update(&stderr_buf[..read]);"
)
assert old_stderr_reset in s
s = s.replace(
    old_stderr_reset,
    "                    } else {\n"
    "                        // PR#3a: real stderr read resets the idle deadline.\n"
    "                        idle_deadline = tokio::time::Instant::now()\n"
    "                            + Duration::from_millis(idle_timeout_ms);\n"
    "                        idle_sleep.as_mut().reset(idle_deadline);\n"
    "                        // PR#2: hash + total BEFORE the cap\n"
    "                        stderr_total_bytes += read as u64;\n"
    "                        stderr_hasher.update(&stderr_buf[..read]);",
)

if s == orig:
    print("NO CHANGES", file=sys.stderr); sys.exit(1)

p.write_text(s)
print("PATCHED OK (PR#3a)")
