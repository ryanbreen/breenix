/* Black-box replay of Breenix-authored command streams through Linux DRM.
 * Build:
 *   cc -O2 -Wall -Wextra -Werror $(pkg-config --cflags libdrm) \
 *     vertex-reuse.c -o vertex-reuse
 * Run from a directory containing the generated streams; see provenance B21.
 * No external implementation, captured external stream or driver algorithm is used.
 */
#include <drm.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <virtgpu_drm.h>
#define N 12
static int fd;
static uint32_t bos[N], ids[N];
static void ck(int r, const char *s) {
  if (r < 0) {
    perror(s);
    exit(2);
  }
}
static void make(unsigned n, uint32_t target, uint32_t format, uint32_t bind,
                 uint32_t w, uint32_t h) {
  struct drm_virtgpu_resource_create c = {.target = target,
                                          .format = format,
                                          .bind = bind,
                                          .width = w,
                                          .height = h,
                                          .depth = 1,
                                          .array_size = 1,
                                          .size = w * h * 4};
  ck(ioctl(fd, DRM_IOCTL_VIRTGPU_RESOURCE_CREATE, &c), "CREATE");
  bos[n] = c.bo_handle;
  ids[n] = c.res_handle;
}
static void wait_all(void) {
  for (unsigned n = 0; n < N; n++) {
    struct drm_virtgpu_3d_wait w = {.handle = bos[n]};
    ck(ioctl(fd, DRM_IOCTL_VIRTGPU_WAIT, &w), "WAIT");
  }
}
static void submit(const char *file) {
  FILE *f = fopen(file, "rb");
  if (!f) {
    perror(file);
    exit(2);
  }
  uint32_t count;
  if (fread(&count, 4, 1, f) != 1)
    exit(2);
  for (unsigned n = 0; n < count; n++) {
    uint32_t len;
    if (fread(&len, 4, 1, f) != 1)
      exit(2);
    uint32_t *buf = malloc(len * 4);
    if (fread(buf, 4, len, f) != len)
      exit(2);
    struct drm_virtgpu_execbuffer e = {.size = len * 4,
                                       .command = (uintptr_t)buf,
                                       .bo_handles = (uintptr_t)bos,
                                       .num_bo_handles = N,
                                       .fence_fd = -1};
    ck(ioctl(fd, DRM_IOCTL_VIRTGPU_EXECBUFFER, &e), "EXEC");
    free(buf);
  }
  fclose(f);
  wait_all();
}

int main(void) {
  setbuf(stdout, NULL);
  fd = open("/dev/dri/renderD128", O_RDWR);
  ck(fd, "OPEN");
  make(0, 0, 64, 1 << 4, 65536, 1);
  make(1, 2, 1, 10, 1, 1);
  make(2, 2, 1, 10, 1, 1);
  make(3, 2, 1, 10, 128, 128);
  make(4, 2, 67, 10, 16384, 16);
  for (unsigned i = 5; i < 11; i++)
    make(i, 2, 1, 10, 64, 128);
  make(11, 2, 1, 10, 128, 128);
  FILE *f = fopen("ids", "w");
  for (unsigned i = 0; i < N; i++)
    fprintf(f, "%u\n", ids[i]);
  fclose(f);
  puts("RESOURCES READY");
  while (access("commands-ready", F_OK))
    sleep(1);
  submit("warm.bin");
  unsigned total = 0;
  for (unsigned frame = 0; frame < 20; frame++) {
    char file[80];
    snprintf(file, sizeof file, "frame-%u-glyph.bin", frame);
    submit(file);
    snprintf(file, sizeof file, "frame-%u-sample.bin", frame);
    submit(file);
    snprintf(file, sizeof file, "frame-%u-expected.bin", frame);
    FILE *expected = fopen(file, "rb");
    if (!expected)
      exit(2);
    unsigned wrong = 0;
    for (unsigned i = 5; i < 11; i++) {
      struct drm_virtgpu_3d_transfer_from_host t = {
          .bo_handle = bos[i], .box = {.w = 64, .h = 128, .d = 1}};
      ck(ioctl(fd, DRM_IOCTL_VIRTGPU_TRANSFER_FROM_HOST, &t), "FROM_HOST");
      wait_all();
      struct drm_virtgpu_map m = {.handle = bos[i]};
      ck(ioctl(fd, DRM_IOCTL_VIRTGPU_MAP, &m), "MAP");
      unsigned char *v = mmap(NULL, 32768, PROT_READ, MAP_SHARED, fd, m.offset);
      if (v == MAP_FAILED) {
        perror("mmap");
        exit(2);
      }
      unsigned count = 0;
      for (unsigned j = 0; j < 32768; j++) {
        int e = fgetc(expected);
        if (e == EOF)
          exit(2);
        if (j % 4 < 3 && v[j] != e)
          count++;
      }
      munmap(v, 32768);
      if (count)
        printf("frame=%u glyph=%u mismatch-components=%u\n", frame, i - 5,
               count);
      wrong += count;
    }
    fclose(expected);
    printf("FRAME %u mismatches=%u\n", frame, wrong);
    total += wrong;
  }
  printf("LINUX REPEATED-UPLOAD PROBE %s mismatches=%u\n",
         total ? "FAIL" : "PASS", total);
  close(fd);
  return total ? 1 : 0;
}
