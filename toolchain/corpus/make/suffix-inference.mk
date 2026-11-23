.SUFFIXES:
.SUFFIXES: .src .out

.src.out:
	@printf '%s\n' '$*' > '$@'
	@cat '$<' >> '$@'
