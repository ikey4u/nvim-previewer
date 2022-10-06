# nvim-rs - neovim plugin framework for rust

- Development

    There is a `runner` plugin for neovim is placed in `examples/vim/runner` directory,
    and the corresponding rust binary source is `examples/runner.rs`.

    To use the plugin, you first need build the 

        cargo build --example runner

    Then start nvim with the plugin, use the following command

        NVIM_PLUGIN_RUNNER=target/debug/examples/runner nvim -u examples/vim/vimrc

    To make the development easy, you can run `make dev` from this project root.

- References

    - [neovim-lib](https://github.com/daa84/neovim-lib/tree/5291bf754bcfa55dcf6332808f72d09ebd78ce90)
    - [neovim-calculator](https://github.com/srishanbhattarai/neovim-calculator/tree/b7cb619f2ca5d1f5bffd270ba535ffe022cfbe33)
