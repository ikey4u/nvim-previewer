function! s:connect(...)
    let s:bin = a:1
    if !filereadable(s:bin)
        echoerr printf('failed to find binary in path: %s', s:bin)
        return 0
    endif

    let jobid = jobstart([s:bin], { 'rpc': v:true })
    if jobid == 0
        echoerr printf('failed to connect to the rpc endpoint: [%s]', s:bin)
    elseif jobid == -1
        echoerr printf('binary [%s] is not executable', s:bin)
    else
        return jobid
    endif
endfunction

if exists("$NVIM_PREVIEWER_PLUGIN_PATH")
    let s:bin = $NVIM_PREVIEWER_PLUGIN_PATH
else
    let s:bin = expand("<sfile>:p:h:h") . '/target/release/nvim-previewer'
endif
" nvim-previewer binary will use this variable as the default style directory
let g:nvim_previewer_script_dir = expand("<sfile>:p:h")
if exists('s:jobid') && s:jobid > 0
    finish
endif
if !filereadable(s:bin)
    echoerr printf('plugin binary is not found')
    finish
endif
let s:jobid = s:connect(s:bin)
if s:jobid <= 0
    echoerr 'failed to start runner plugin'
    finish
endif

command! -nargs=0 Preview call rpcnotify(s:jobid, 'preview', expand('%:p'))
