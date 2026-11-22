all: fail
	@printf 'dependent ran\n'

fail:
	@printf 'before failure\n'
	@exit 7
	@printf 'recipe continued\n'
