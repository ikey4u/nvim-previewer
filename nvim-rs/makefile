PROJDIR=$(shell git rev-parse --show-toplevel)/nvim-rs

dev:
	@cargo build --example runner && NVIM_PLUGIN_RUNNER=${PROJDIR}/target/debug/examples/runner nvim -u ${PROJDIR}/examples/vim/vimrc target/test.txt
