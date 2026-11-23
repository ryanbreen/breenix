CC=cc
AS=as

project: main.o util.o value.o
	$(CC) -o $@ main.o util.o value.o

main.o util.o: util.h

value.o: value.s
	$(AS) -o $@ value.s

.c.o:
	$(CC) -c -o $@ $<
