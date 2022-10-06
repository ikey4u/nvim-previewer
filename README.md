# nvim-previewer

A nvim file previewer written in Rust, it only supports markdown file preview for now.

You can install this plugin with [vim-plug](https://github.com/junegunn/vim-plug) using the following
configuration

    Plug 'ikey4u/nvim-previewer', { 'do': 'cargo build --release' }

To cutmoize the broswer and listening port, using these options

    let g:nvim_previewer_browser = "firefox"
    let g:nvim_previewer_port = 3008
