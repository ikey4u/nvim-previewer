dev:
	@cargo build && NVIM_PREVIEWER_PLUGIN_PATH=target/debug/nvim-previewer nvim -u vimrc README.md
