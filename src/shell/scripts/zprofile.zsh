ZDOTDIR=$GITOUI_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zprofile ]] && builtin source $ZDOTDIR/.zprofile
ZDOTDIR=$__gitoui_inject_dir
