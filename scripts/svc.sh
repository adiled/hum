#!/bin/bash
# scripts/svc.sh — single point of OS dispatch for user-level
# service management. Sourced by every installer that registers a
# long-running process.
#
# Linux: systemd --user units in $XDG_CONFIG_HOME/systemd/user.
# macOS: launchd LaunchAgents in ~/Library/LaunchAgents.
# Other: graceful warn + return failure; caller decides whether to
#        bail or fall back to a paradigm-0 "run it yourself" hint.
#
# Usage:
#   source scripts/svc.sh
#   svc_install <name> <exec_with_args> [--env KEY=VAL]...
#   svc_start    <name>
#   svc_restart  <name>
#   svc_stop     <name>
#   svc_status   <name>
#   svc_is_active <name>   # 0 if running, 1 otherwise
#   svc_uninstall <name>
#
# `name` is the canonical short id (e.g. "hum", "hum-openai-server").
# The helper picks the right unit filename per OS.

SVC_OS="$(uname -s)"

# ─── path resolvers ──────────────────────────────────────────────────────

_svc_unit_linux()  { echo "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$1.service"; }
_svc_unit_darwin() { echo "$HOME/Library/LaunchAgents/sh.hum.$1.plist"; }
_svc_label_darwin(){ echo "sh.hum.$1"; }

# ─── install ────────────────────────────────────────────────────────────
# svc_install <name> <exec_line> [--env KEY=VAL]...
# exec_line: full command string ("/path/to/bin --flag")
# --env:     repeat as needed to inject Environment / EnvironmentVariables
svc_install() {
  local name="$1"; shift
  local exec_line="$1"; shift
  local envs=()
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --env) envs+=("$2"); shift 2 ;;
      *) shift ;;
    esac
  done
  case "$SVC_OS" in
    Linux)  _svc_install_linux  "$name" "$exec_line" "${envs[@]}" ;;
    Darwin) _svc_install_darwin "$name" "$exec_line" "${envs[@]}" ;;
    *) echo "svc: unsupported OS '$SVC_OS' — run '$exec_line' manually" >&2; return 1 ;;
  esac
}

_svc_install_linux() {
  local name="$1"; shift
  local exec_line="$1"; shift
  local unit; unit="$(_svc_unit_linux "$name")"
  mkdir -p "$(dirname "$unit")"
  {
    echo "[Unit]"
    echo "Description=hum service: $name"
    echo "After=network.target"
    echo ""
    echo "[Service]"
    echo "Type=simple"
    for kv in "$@"; do echo "Environment=$kv"; done
    echo "ExecStart=$exec_line"
    echo "Restart=on-failure"
    echo "RestartSec=2s"
    echo "StandardOutput=journal"
    echo "StandardError=journal"
    echo "SyslogIdentifier=$name"
    echo ""
    echo "[Install]"
    echo "WantedBy=default.target"
  } > "$unit"
  systemctl --user daemon-reload 2>/dev/null || true
  systemctl --user enable "$name" 2>/dev/null || true
}

_svc_install_darwin() {
  local name="$1"; shift
  local exec_line="$1"; shift
  local plist; plist="$(_svc_unit_darwin "$name")"
  local label; label="$(_svc_label_darwin "$name")"
  mkdir -p "$(dirname "$plist")"
  # Split exec_line into argv, properly handling quoted args.
  # Simpler: write a shell wrapper line as `ProgramArguments` via /bin/sh -c
  # so users can pass complex flags without our parser fighting them.
  {
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
    echo '<plist version="1.0">'
    echo '<dict>'
    echo "  <key>Label</key><string>$label</string>"
    echo '  <key>ProgramArguments</key>'
    echo '  <array>'
    echo '    <string>/bin/sh</string>'
    echo '    <string>-c</string>'
    echo "    <string>$exec_line</string>"
    echo '  </array>'
    echo '  <key>RunAtLoad</key><true/>'
    echo '  <key>KeepAlive</key><true/>'
    if [ "$#" -gt 0 ]; then
      echo '  <key>EnvironmentVariables</key>'
      echo '  <dict>'
      for kv in "$@"; do
        local k="${kv%%=*}"; local v="${kv#*=}"
        echo "    <key>$k</key><string>$v</string>"
      done
      echo '  </dict>'
    fi
    echo '  <key>StandardOutPath</key>'
    echo "  <string>$HOME/Library/Logs/$label.out.log</string>"
    echo '  <key>StandardErrorPath</key>'
    echo "  <string>$HOME/Library/Logs/$label.err.log</string>"
    echo '</dict>'
    echo '</plist>'
  } > "$plist"
}

# ─── runtime control ────────────────────────────────────────────────────

svc_start() {
  case "$SVC_OS" in
    Linux)  systemctl --user start "$1" 2>/dev/null ;;
    Darwin) launchctl bootstrap "gui/$(id -u)" "$(_svc_unit_darwin "$1")" 2>/dev/null || \
            launchctl kickstart "gui/$(id -u)/$(_svc_label_darwin "$1")" 2>/dev/null ;;
    *) return 1 ;;
  esac
}

svc_restart() {
  case "$SVC_OS" in
    Linux)  systemctl --user restart "$1" ;;
    Darwin) launchctl bootout "gui/$(id -u)/$(_svc_label_darwin "$1")" 2>/dev/null
            launchctl bootstrap "gui/$(id -u)" "$(_svc_unit_darwin "$1")" ;;
    *) return 1 ;;
  esac
}

svc_stop() {
  case "$SVC_OS" in
    Linux)  systemctl --user stop "$1" 2>/dev/null ;;
    Darwin) launchctl bootout "gui/$(id -u)/$(_svc_label_darwin "$1")" 2>/dev/null ;;
    *) return 1 ;;
  esac
}

svc_status() {
  case "$SVC_OS" in
    Linux)  systemctl --user status "$1" --no-pager 2>&1 | head -8 ;;
    Darwin) launchctl print "gui/$(id -u)/$(_svc_label_darwin "$1")" 2>&1 | head -20 ;;
    *) echo "svc: unsupported OS '$SVC_OS'" ;;
  esac
}

svc_is_active() {
  case "$SVC_OS" in
    Linux)  systemctl --user is-active --quiet "$1" ;;
    Darwin) launchctl print "gui/$(id -u)/$(_svc_label_darwin "$1")" 2>/dev/null | grep -q 'state = running' ;;
    *) return 1 ;;
  esac
}

svc_uninstall() {
  case "$SVC_OS" in
    Linux)
      systemctl --user stop "$1" 2>/dev/null || true
      systemctl --user disable "$1" 2>/dev/null || true
      rm -f "$(_svc_unit_linux "$1")"
      # Sibling timer, if any.
      systemctl --user stop "$1.timer" 2>/dev/null || true
      systemctl --user disable "$1.timer" 2>/dev/null || true
      rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$1.timer"
      systemctl --user daemon-reload 2>/dev/null || true
      ;;
    Darwin)
      launchctl bootout "gui/$(id -u)/$(_svc_label_darwin "$1")" 2>/dev/null || true
      rm -f "$(_svc_unit_darwin "$1")"
      ;;
    *) return 1 ;;
  esac
}

# ─── periodic timer ─────────────────────────────────────────────────────
# svc_timer_install <name> <on_calendar> <exec_line> [--env KEY=VAL]...
# Installs a recurring trigger that fires <exec_line>. <on_calendar>
# uses systemd OnCalendar syntax on Linux (e.g. "daily", "weekly",
# "06:00", "Mon *-*-* 06:00:00"). On macOS, we approximate by
# scheduling a launchd job with StartCalendarInterval — only the
# common rules (daily / weekly / hourly) are translated; anything
# more specific falls back to "daily 03:00 local".
svc_timer_install() {
  local name="$1"; shift
  local on_calendar="$1"; shift
  local exec_line="$1"; shift
  local envs=()
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --env) envs+=("$2"); shift 2 ;;
      *) shift ;;
    esac
  done
  case "$SVC_OS" in
    Linux)
      local svc_unit; svc_unit="$(_svc_unit_linux "$name")"
      local tmr_unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$name.timer"
      mkdir -p "$(dirname "$svc_unit")"
      # The service unit fires once per timer trigger (Type=oneshot).
      {
        echo "[Unit]"
        echo "Description=hum timer-driven job: $name"
        echo ""
        echo "[Service]"
        echo "Type=oneshot"
        for kv in "${envs[@]}"; do echo "Environment=$kv"; done
        echo "ExecStart=$exec_line"
      } > "$svc_unit"
      {
        echo "[Unit]"
        echo "Description=hum timer: $name (every $on_calendar)"
        echo ""
        echo "[Timer]"
        echo "OnCalendar=$on_calendar"
        echo "Persistent=true"
        echo ""
        echo "[Install]"
        echo "WantedBy=timers.target"
      } > "$tmr_unit"
      systemctl --user daemon-reload 2>/dev/null || true
      systemctl --user enable --now "$name.timer" 2>/dev/null || true
      ;;
    Darwin)
      local plist; plist="$(_svc_unit_darwin "$name")"
      local label; label="$(_svc_label_darwin "$name")"
      # Approximate calendar rules — daily 03:00 is the catch-all.
      local hour=3 minute=0
      case "$on_calendar" in
        hourly) hour=""; minute=0 ;;
        weekly) hour=3; minute=0 ;;
        *) hour=3; minute=0 ;;
      esac
      mkdir -p "$(dirname "$plist")"
      {
        echo '<?xml version="1.0" encoding="UTF-8"?>'
        echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
        echo '<plist version="1.0">'
        echo '<dict>'
        echo "  <key>Label</key><string>$label</string>"
        echo '  <key>ProgramArguments</key>'
        echo '  <array>'
        echo '    <string>/bin/sh</string>'
        echo '    <string>-c</string>'
        echo "    <string>$exec_line</string>"
        echo '  </array>'
        echo '  <key>StartCalendarInterval</key>'
        echo '  <dict>'
        if [ -n "$hour" ]; then echo "    <key>Hour</key><integer>$hour</integer>"; fi
        echo "    <key>Minute</key><integer>$minute</integer>"
        echo '  </dict>'
        if [ "${#envs[@]}" -gt 0 ]; then
          echo '  <key>EnvironmentVariables</key>'
          echo '  <dict>'
          for kv in "${envs[@]}"; do
            local k="${kv%%=*}"; local v="${kv#*=}"
            echo "    <key>$k</key><string>$v</string>"
          done
          echo '  </dict>'
        fi
        echo '  <key>StandardOutPath</key>'
        echo "  <string>$HOME/Library/Logs/$label.out.log</string>"
        echo '  <key>StandardErrorPath</key>'
        echo "  <string>$HOME/Library/Logs/$label.err.log</string>"
        echo '</dict>'
        echo '</plist>'
      } > "$plist"
      launchctl bootstrap "gui/$(id -u)" "$plist" 2>/dev/null || true
      ;;
    *) return 1 ;;
  esac
}
