esc="$(printf '\033')"
use_color=0
if [[ -z "${NO_COLOR:-}" ]]; then
  if [[ -n "${FORCE_COLOR:-}" ]]; then
    [[ "${FORCE_COLOR}" != "0" ]] && use_color=1
  elif [[ -t 1 ]]; then
    use_color=1
  fi
fi

paint() {
  local code="$1"
  local text="$2"
  if [[ "$use_color" -eq 1 ]]; then
    printf '%s[%sm%s%s[0m' "$esc" "$code" "$text" "$esc"
  else
    printf '%s' "$text"
  fi
}

green() { paint 32 "$1"; }
red() { paint 31 "$1"; }
dim() { paint 2 "$1"; }
bold_red() { paint '1;31' "$1"; }

strip_ansi() {
  sed -E $'s/\x1b\\[[0-9;?]*[ -/]*[@-~]//g; s/\r//g'
}

elapsed_seconds() {
  local started="$1"
  local ended
  ended="$(date +%s)"
  printf '%ss' "$((ended - started))"
}
