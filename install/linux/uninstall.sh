#!/usr/bin/env bash
# Removes worklog installed by install.sh: the systemd timer, the menu entry, the icons and the
# binaries. Settings and history in ~/.config/worklog stay unless PURGE=1.
#
#   PREFIX=~/.local     as given to install.sh
#   PURGE=1             also remove ~/.config/worklog
#   NO_SCHEDULE=1       leave the systemd timer alone
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
bin_dir="$PREFIX/bin"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
[ "$PREFIX" = "$HOME/.local" ] || data_home="$PREFIX/share"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/worklog"

if [ -t 1 ]; then
    c_step=$'\e[1;36m' c_ok=$'\e[32m' c_warn=$'\e[33m' c_dim=$'\e[2m' c_off=$'\e[0m'
else
    c_step='' c_ok='' c_warn='' c_dim='' c_off=''
fi
step() { printf '%s==>%s %s\n' "$c_step" "$c_off" "$1"; }
ok()   { printf '    %s✓%s %s\n' "$c_ok" "$c_off" "$1"; }
warn() { printf '    %s!%s %s\n' "$c_warn" "$c_off" "$1"; }

remove() {
    if [ -e "$1" ]; then rm -f "$1"; ok "removed $1"; fi
}

if [ "${NO_SCHEDULE:-0}" != 1 ]; then
    step "Daily prompt"
    if [ -x "$bin_dir/worklog" ] && "$bin_dir/worklog" uninstall >/dev/null 2>&1; then
        ok "removed worklog.timer"
    elif command -v systemctl >/dev/null 2>&1; then
        # `worklog uninstall` reads the settings first; broken settings must not leave the timer.
        systemctl --user disable --now worklog.timer >/dev/null 2>&1 || true
        rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/worklog."{service,timer}
        systemctl --user daemon-reload >/dev/null 2>&1 || true
        ok "no timer left"
    else
        ok "no systemd here"
    fi
fi

step "Application menu and icons"
remove "$data_home/applications/worklog.desktop"
remove "$data_home/icons/hicolor/scalable/apps/worklog.svg"
remove "$data_home/icons/hicolor/256x256/apps/worklog.png"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data_home/applications" || true

step "Program"
remove "$bin_dir/worklog"
remove "$bin_dir/worklog-popup"
rm -rf "$data_home/worklog"
ok "done"

if [ "${PURGE:-0}" = 1 ]; then
    step "Settings and history"
    rm -rf "$config_dir"
    ok "removed $config_dir"
else
    printf '    %ssettings and history kept in %s (PURGE=1 removes them)%s\n' "$c_dim" "$config_dir" "$c_off"
fi
printf '\n  %sworklog is uninstalled.%s\n' "$c_ok" "$c_off"
