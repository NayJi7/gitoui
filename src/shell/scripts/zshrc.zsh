ZDOTDIR=$GITOUI_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zshrc ]] && builtin source $ZDOTDIR/.zshrc
ZDOTDIR=$__gitoui_inject_dir
builtin source $__gitoui_inject_dir/gitoui.zsh
