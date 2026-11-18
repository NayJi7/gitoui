set -g __gitoui_nonce $GITOUI_NONCE
set -e GITOUI_NONCE
function __gitoui_cwd --on-event fish_prompt
    printf '\033]7777;gitoui;%s;C;%s\007' $__gitoui_nonce $PWD >/dev/tty
end
