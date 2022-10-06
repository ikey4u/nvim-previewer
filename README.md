# nvim-previewer

## Introduction

A nvim file previewer written in Rust, it only supports markdown file preview for now.

## Installation

You should install these prerequisites firstly

- [nvim](https://neovim.io/) >= 0.7.0
- [rust](https://www.rust-lang.org/tools/install) >= 1.64

Then install this plugin with [vim-plug](https://github.com/junegunn/vim-plug) using the following
configuration

    Plug 'ikey4u/nvim-previewer', { 'do': 'cargo build --release' }

To cutmoize the broswer and listening port, using these options

    let g:nvim_previewer_browser = "firefox"
    let g:nvim_previewer_port = 3008

## Usage

Run `:Preview` in your markdown file, and you are done.

