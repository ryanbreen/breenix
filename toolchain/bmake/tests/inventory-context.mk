	outside := value
# $(error ignored) \
continued_comment += $(error ignored)
include missing.mk
include = value
ifdef FLAG
else ifneq (a=b,c:d)
override define template +=
.PHONY: body
body := $(subst #,x,text)
	endef
endef
endif
objects: %.o: %.c .WAIT ready
%.o: private export FLAGS += $(strip \
	${filter %.c,$(sources)})
ordinary: $(var:%.c=%.o) ; @echo $(info recipe) X=y # $(file >marker,bad)
	ifdef shell_text \
	$(warning continued recipe) $$(error escaped)
escaped\%: ready\#name # $(error ignored)
.PHONY: ordinary
value :::= $($(name):%.c=%.o)
