function! runner#connect(...)
    let s:bin = a:1
    if !filereadable(s:bin)
        echoerr printf('failed to find runner binary in path: %s', s:bin)
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

function! runner#serve(...) range
    let s:jobid = a:1
    call rpcnotify(s:jobid, 'run', str2nr(a:firstline), str2nr(a:lastline))
endfunction
