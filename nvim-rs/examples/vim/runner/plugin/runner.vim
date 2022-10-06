if exists('s:jobid') && s:jobid > 0
    finish
endif

let s:bin=$NVIM_PLUGIN_RUNNER
if !filereadable(s:bin)
    echoerr printf('Runner plugin binary is not found, please check NVIM_PLUGIN_RUNNER and try again')
    finish
endif

let s:jobid = runner#connect(s:bin)
if s:jobid <= 0
    echoerr 'failed to start runner plugin'
    finish
endif

command! -range Run <line1>,<line2>call runner#serve(s:jobid)
