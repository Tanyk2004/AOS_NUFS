// small_write_close_four.c
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>

static inline uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}
static inline double ns_to_ms(uint64_t ns) { return (double)ns / 1e6; }
static inline double ns_to_s(uint64_t ns)  { return (double)ns / 1e9; }

typedef struct {
    double open_ms, write_ms, close_ms, total_ms;
    ssize_t bytes_written;
    off_t   filesize;
} metrics_t;

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr,
            "Usage: %s <base_path> [write_size_B=4096] [offset_B=0]\n"
            "Example: %s /mnt/netfs/bigfile 4096 0   # will use bigfile_0 .. bigfile_3\n",
            argv[0], argv[0]);
        return 2;
    }
    const char *base = argv[1];
    size_t write_sz = (argc >= 3) ? (size_t)atoll(argv[2]) : 4096;
    off_t offset    = (argc >= 4) ? (off_t)atoll(argv[3]) : 0;

    char *buf = (char*)malloc(write_sz);
    if (!buf) { perror("malloc"); return 1; }
    for (size_t i = 0; i < write_sz; ++i) buf[i] = (char)(0xBA ^ (i & 0xFF));

    metrics_t m[4] = {0};
    double sum_open=0, sum_write=0, sum_close=0, sum_total=0;

    for (int i = 0; i < 4; ++i) {
        char path[4096];
        snprintf(path, sizeof(path), "%s_%d", base, i);

        uint64_t t_total0 = now_ns();

        uint64_t t_open0 = now_ns();
        int fd = open(path, O_RDWR);
        uint64_t t_open1 = now_ns();
        if (fd < 0) {
            fprintf(stderr, "open('%s') failed: %s\n", path, strerror(errno));
            free(buf);
            return 1;
        }

        struct stat st;
        if (fstat(fd, &st) != 0) {
            fprintf(stderr, "fstat('%s') failed: %s\n", path, strerror(errno));
            close(fd); free(buf); return 1;
        }
        off_t filesize = st.st_size;

        if (filesize > 0 && offset + (off_t)write_sz > filesize) {
            // keep write within file; fall back to 0
            offset = 0;
        }

        uint64_t t_write0 = now_ns();
        if (lseek(fd, offset, SEEK_SET) == (off_t)-1) {
            fprintf(stderr, "lseek('%s', off=%lld) failed: %s\n",
                    path, (long long)offset, strerror(errno));
            close(fd); free(buf); return 1;
        }
        ssize_t nb = write(fd, buf, write_sz);
        uint64_t t_write1 = now_ns();
        if (nb != (ssize_t)write_sz) {
            if (nb < 0) fprintf(stderr, "write('%s') failed: %s\n", path, strerror(errno));
            else fprintf(stderr, "short write on '%s': %zd/%zu bytes\n", path, nb, write_sz);
            close(fd); free(buf); return 1;
        }

        uint64_t t_close0 = now_ns();
        int rc = close(fd);
        uint64_t t_close1 = now_ns();
        if (rc != 0) {
            fprintf(stderr, "close('%s') failed: %s\n", path, strerror(errno));
            free(buf); return 1;
        }

        uint64_t t_total1 = now_ns();

        m[i].open_ms  = ns_to_ms(t_open1  - t_open0);
        m[i].write_ms = ns_to_ms(t_write1 - t_write0);
        m[i].close_ms = ns_to_ms(t_close1 - t_close0);
        m[i].total_ms = ns_to_ms(t_total1 - t_total0);
        m[i].bytes_written = nb;
        m[i].filesize = filesize;

        sum_open  += m[i].open_ms;
        sum_write += m[i].write_ms;
        sum_close += m[i].close_ms;
        sum_total += m[i].total_ms;

        printf("File %d: %s\n", i, path);
        printf("  FileSize: %lld bytes, Write: %zu bytes at off=%lld\n",
               (long long)filesize, write_sz, (long long)offset);
        printf("  open:  %.3f ms\n", m[i].open_ms);
        printf("  write: %.3f ms\n", m[i].write_ms);
        printf("  close: %.3f ms\n", m[i].close_ms);
        printf("  total: %.3f ms\n\n", m[i].total_ms);

        fprintf(stderr,
            "CSV,file,%s,index,%d,filesize,%lld,write_bytes,%zu,offset,%lld,open_ms,%.3f,write_ms,%.3f,close_ms,%.3f,total_ms,%.3f\n",
            path, i, (long long)filesize, write_sz, (long long)offset,
            m[i].open_ms, m[i].write_ms, m[i].close_ms, m[i].total_ms);
    }

    printf("Summary (4 files): write=%zu B at off=%lld\n", write_sz, (long long)offset);
    printf("  avg open:  %.3f ms\n", sum_open  / 4.0);
    printf("  avg write: %.3f ms\n", sum_write / 4.0);
    printf("  avg close: %.3f ms\n", sum_close / 4.0);
    printf("  avg total: %.3f ms\n", sum_total / 4.0);

    free(buf);
    return 0;
}
