# gitoui command mode: $ZDOTDIR points here. Each startup file loads the
# user's own copy, then hands back to this directory.
__gitoui_inject_dir=$ZDOTDIR
ZDOTDIR=${GITOUI_USER_ZDOTDIR:-$HOME}
[[ -f $ZDOTDIR/.zshenv ]] && builtin source $ZDOTDIR/.zshenv
GITOUI_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$__gitoui_inject_dir
