let curdir = expand('<sfile>:p:h')
exec printf('set runtimepath=%s,%s', $VIMRUNTIME, curdir)
let g:nvim_previewer_port = 3009
let g:nvim_previewer_browser = 'firefox'
