if [[ -n ${GITOUI_LOGIN-} ]]; then
  [[ -r /etc/profile ]] && builtin source /etc/profile
  if [[ -r ~/.bash_profile ]]; then builtin source ~/.bash_profile
  elif [[ -r ~/.bash_login ]]; then builtin source ~/.bash_login
  elif [[ -r ~/.profile ]]; then builtin source ~/.profile
  fi
else
  [[ -r ~/.bashrc ]] && builtin source ~/.bashrc
fi
__gitoui_nonce=$GITOUI_NONCE
builtin unset GITOUI_NONCE GITOUI_LOGIN
__gitoui_prompt() {
  local s=$?
  builtin printf '\033]7777;gitoui;%s;C;%s\007' "$__gitoui_nonce" "$PWD" >/dev/tty
  return $s
}
PROMPT_COMMAND="__gitoui_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
