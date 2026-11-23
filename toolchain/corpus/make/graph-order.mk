all: left right
	@printf 'all\n'

left: shared
	@printf 'left\n'

right: shared
	@printf 'right\n'

shared:
	@printf 'shared\n'
