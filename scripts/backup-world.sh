#!/usr/bin/env bash
# Back up Folia-RS world data to a timestamped tar.gz and prune old backups.
#
# Env vars (all optional):
#   ROOT_DIR        install/repo root (default: parent of this script's dir)
#   WORLD_DIR       level directory (default: "$ROOT_DIR/world")
#   PLAYERDATA_DIR  playerdata directory (default: "$ROOT_DIR/run/playerdata")
#   BACKUP_DIR      destination directory (default: "$ROOT_DIR/backups")
#   KEEP            number of newest backups to retain (default: 5)
#
# Stop the server (or type `stop` in the console) first so terrain and level.dat
# are flushed. Only files inside BACKUP_DIR are ever deleted.

set -euo pipefail

ROOT_DIR="${ROOT_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)}"
WORLD_DIR="${WORLD_DIR:-$ROOT_DIR/world}"
PLAYERDATA_DIR="${PLAYERDATA_DIR:-$ROOT_DIR/run/playerdata}"
BACKUP_DIR="${BACKUP_DIR:-$ROOT_DIR/backups}"
KEEP="${KEEP:-5}"

die() { printf 'backup-world: %s\n' "$*" >&2; exit 1; }

# --- Validate inputs -------------------------------------------------------
[[ -d "$WORLD_DIR" ]] || die "world directory not found: $WORLD_DIR"
[[ -d "$WORLD_DIR/region" ]] || die "region directory not found: $WORLD_DIR/region"
[[ -d "$PLAYERDATA_DIR" ]] || die "playerdata directory not found: $PLAYERDATA_DIR"
[[ "$KEEP" =~ ^[0-9]+$ ]] || die "KEEP must be a non-negative integer, got: $KEEP"
(( KEEP >= 1 )) || die "KEEP must be at least 1, got: $KEEP"
[[ -n "$BACKUP_DIR" && "$BACKUP_DIR" != "/" ]] || die "BACKUP_DIR must not be empty or /"

WORLD_DIR="$(cd -- "$WORLD_DIR" && pwd)"
PLAYERDATA_DIR="$(cd -- "$PLAYERDATA_DIR" && pwd)"
mkdir -p -- "$BACKUP_DIR"
BACKUP_DIR="$(cd -- "$BACKUP_DIR" && pwd)"

# Never stage or prune inside the data being backed up (recursion / data loss).
case "$BACKUP_DIR" in
  "$WORLD_DIR"|"$WORLD_DIR"/*|"$PLAYERDATA_DIR"|"$PLAYERDATA_DIR"/*)
    die "BACKUP_DIR must be outside WORLD_DIR and PLAYERDATA_DIR: $BACKUP_DIR" ;;
esac
[[ -w "$BACKUP_DIR" ]] || die "backup directory is not writable: $BACKUP_DIR"

# --- Create the archive ----------------------------------------------------
timestamp="$(date +%Y%m%d-%H%M%S)"
archive="$BACKUP_DIR/folia-world-$timestamp.tar.gz"
[[ ! -e "$archive" ]] || die "refusing to overwrite existing archive: $archive"

level="$WORLD_DIR/level.dat"
[[ -f "$level" ]] || printf 'backup-world: warning: no level.dat at %s\n' "$level" >&2

# Symlinks give clean archive paths without copying large world files; tar -h
# dereferences them so the real files/directories are stored.
stage="$BACKUP_DIR/.stage.$$"
cleanup() { rm -rf -- "$stage"; }
trap cleanup EXIT
mkdir -p -- "$stage"
ln -s -- "$WORLD_DIR/region" "$stage/region"
ln -s -- "$PLAYERDATA_DIR" "$stage/playerdata"

sources=(region playerdata)
if [[ -f "$level" ]]; then
  ln -s -- "$level" "$stage/level.dat"
  sources+=(level.dat)
fi

tar -hczf "$archive" -C "$stage" -- "${sources[@]}"
printf 'backup-world: created %s\n' "$archive"

# --- Prune to the KEEP newest archives (only inside BACKUP_DIR) ------------
shopt -s nullglob
backups=("$BACKUP_DIR"/folia-world-*.tar.gz)
if (( ${#backups[@]} > KEEP )); then
  mapfile -t sorted < <(ls -1t -- "${backups[@]}")
  for ((i = KEEP; i < ${#sorted[@]}; i++)); do
    rm -f -- "${sorted[i]}"
    printf 'backup-world: pruned %s\n' "${sorted[i]}"
  done
fi
