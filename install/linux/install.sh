#!/usr/bin/env bash
# Installs worklog for the current user (no root): the binaries in ~/.local/bin, the icon and an
# application-menu entry ("Worklog"), and the daily prompt as a systemd user timer.
#
#   ./install/linux/install.sh            run from a clone of the repository
#
# Environment:
#   PREFIX=~/.local     where bin/ and share/ go
#   BUILD=1             rebuild (cargo build --release) even when the binaries exist
#   NO_SCHEDULE=1       skip the systemd timer (add it later with: worklog install)
#   SOURCE=dir          take the binaries from dir instead of target/release
# Running it again updates an existing installation; settings are kept.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
SOURCE="${SOURCE:-$repo/target/release}"
bin_dir="$PREFIX/bin"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
# With a non-default PREFIX the menu entry and icons go there too, as with any other app.
[ "$PREFIX" = "$HOME/.local" ] || data_home="$PREFIX/share"
config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/worklog"
app_dir="$data_home/worklog"

if [ -t 1 ]; then
    c_step=$'\e[1;36m' c_ok=$'\e[32m' c_warn=$'\e[33m' c_err=$'\e[31m' c_dim=$'\e[2m' c_bold=$'\e[1m' c_off=$'\e[0m'
else
    c_step='' c_ok='' c_warn='' c_err='' c_dim='' c_bold='' c_off=''
fi
step() { printf '%s==>%s %s\n' "$c_step" "$c_off" "$1"; }
ok()   { printf '    %s✓%s %s\n' "$c_ok" "$c_off" "$1"; }
warn() { printf '    %s!%s %s\n' "$c_warn" "$c_off" "$1"; }
fail() { printf '    %s✗%s %s\n' "$c_err" "$c_off" "$1" >&2; exit 1; }

printf '\n  %sworklog installer%s\n  %s-> %s%s\n\n' "$c_bold" "$c_off" "$c_dim" "$bin_dir" "$c_off"

step "Checking prerequisites"
command -v git >/dev/null 2>&1 || fail "git is not installed; worklog reads your activity from git"
ok "git $(git --version | awk '{print $3}')"

step "Checking the binaries"
if [ "${BUILD:-0}" = 1 ] || [ ! -x "$SOURCE/worklog" ] || [ ! -x "$SOURCE/worklog-popup" ]; then
    command -v cargo >/dev/null 2>&1 || fail "no build in $SOURCE and cargo is not installed; get Rust from https://rustup.rs and run this again"
    echo "    building the release (a few minutes the first time)..."
    echo "    ${c_dim}the window needs the usual GUI libraries: libxkbcommon, libGL, libwayland or libX11${c_off}"
    (cd "$repo" && cargo build --release) || fail "cargo build --release failed"
fi
for b in worklog worklog-popup; do
    [ -x "$SOURCE/$b" ] || fail "$b not found in $SOURCE"
done
ok "found in $SOURCE"

step "Copying the program"
mkdir -p "$bin_dir" "$app_dir"
for b in worklog worklog-popup; do
    # Through a temporary name: replacing a running binary in place fails with "text file busy".
    install -m 755 "$SOURCE/$b" "$bin_dir/.$b.new"
    mv -f "$bin_dir/.$b.new" "$bin_dir/$b"
done
install -m 755 "$here/uninstall.sh" "$app_dir/uninstall.sh"
ok "$bin_dir/worklog, $bin_dir/worklog-popup"

step "Icon and application menu"
install -D -m 644 "$repo/assets/icon/worklog.svg" "$data_home/icons/hicolor/scalable/apps/worklog.svg"
install -D -m 644 "$repo/assets/icon/worklog.png" "$data_home/icons/hicolor/256x256/apps/worklog.png"
mkdir -p "$data_home/applications"
cat > "$data_home/applications/worklog.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Worklog
GenericName=Jira worklog helper
Comment=Suggest today's Jira worklogs from your git activity
Exec="$bin_dir/worklog" popup --force
Icon=worklog
Terminal=false
Categories=Office;Development;
Keywords=jira;worklog;time;git;
StartupWMClass=worklog
EOF
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$data_home/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$data_home/icons/hicolor" 2>/dev/null || true
ok "$data_home/applications/worklog.desktop"

step "Settings"
mkdir -p "$config_dir"
if [ -f "$config_dir/.env" ]; then
    ok "keeping $config_dir/.env"
elif [ -f "$repo/.env" ]; then
    # The installed binary no longer sits under the repository, where it found this file before.
    install -m 600 "$repo/.env" "$config_dir/.env"
    ok "copied the repository's .env to $config_dir/.env"
else
    install -m 644 "$repo/.env.example" "$config_dir/.env.example"
    warn "no .env yet: copy $config_dir/.env.example to .env there and fill it in (see README)"
fi

step "Daily prompt (systemd user timer)"
if [ "${NO_SCHEDULE:-0}" = 1 ]; then
    warn "skipped (NO_SCHEDULE=1); add it later with: worklog install"
elif ! command -v systemctl >/dev/null 2>&1 || ! systemctl --user show-environment >/dev/null 2>&1; then
    warn "no systemd user session here; start worklog from the menu, or schedule '$bin_dir/worklog-popup' yourself"
elif out="$("$bin_dir/worklog" install 2>&1)"; then
    ok "$(printf '%s\n' "$out" | tail -n 1)"
else
    warn "not scheduled: $out"
    warn "fix the settings, then run: $bin_dir/worklog install"
fi

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) warn "$bin_dir is not on your PATH; add it to use 'worklog' in a terminal" ;;
esac

printf '\n  %sDone.%s Start it from your application menu: Worklog\n' "$c_ok" "$c_off"
printf '  %sUninstall: %s%s\n\n' "$c_dim" "$app_dir/uninstall.sh" "$c_off"
