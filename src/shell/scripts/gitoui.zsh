typeset -g __gitoui_nonce=$GITOUI_NONCE __gitoui_first=1
unset GITOUI_NONCE
__gitoui_precmd() {
  local s=$?
  if (( __gitoui_first )); then
    __gitoui_first=0
    ZDOTDIR=$GITOUI_USER_ZDOTDIR
    # A system /etc/zshrc runs while ZDOTDIR is still our directory and may
    # derive HISTFILE from it: point it at the user's directory instead.
    if [[ -n ${HISTFILE-} && $HISTFILE == $__gitoui_inject_dir/* ]]; then
      HISTFILE=$GITOUI_USER_ZDOTDIR/${HISTFILE:t}
      fc -R "$HISTFILE" 2>/dev/null
    fi
    unset GITOUI_USER_ZDOTDIR
  fi
  builtin printf '\033]7777;gitoui;%s;C;%s\007' "$__gitoui_nonce" "$PWD" >/dev/tty
  return $s
}
precmd_functions=(__gitoui_precmd $precmd_functions)
