ZDOTDIR=$GITOUI_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zlogin ]] && builtin source $ZDOTDIR/.zlogin
ZDOTDIR=$__gitoui_inject_dir
