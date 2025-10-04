// random_writes_timed.c
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

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "Usage: %s <file_path> <iterations>\n", argv[0]);
        fprintf(stderr, "Example: %s /mnt/netfs/bigfile 200000\n", argv[0]);
        return 2;
    }
    const char *path = argv[1];
    long iterations = atol(argv[2]);
    if (iterations <= 0) {
        fprintf(stderr, "iterations must be > 0\n");
        return 2;
    }

    enum { WRITE_SZ = 100 };
    char buf[WRITE_SZ];
    for (int i = 0; i < WRITE_SZ; ++i) buf[i] = (char)(i & 0xFF);

    /* timing markers */
    uint64_t t_total0 = now_ns();

    /* ---- OPEN ---- */
    uint64_t t_open0 = now_ns();
    int fd = open(path, O_RDWR);
    uint64_t t_open1 = now_ns();
    if (fd < 0) {
        fprintf(stderr, "open('%s') failed: %s\n", path, strerror(errno));
        return 1;
    }

    struct stat st;
    if (fstat(fd, &st) != 0) {
        fprintf(stderr, "fstat failed on '%s': %s\n", path, strerror(errno));
        close(fd);
        return 1;
    }
    off_t filesize = st.st_size;
    if (filesize <= 0) {
        /* fall back to 1 GiB logical bound if size unknown/zero */
        filesize = (off_t)1024 * 1024 * 1024;
    }
    off_t max_offset = (filesize > WRITE_SZ) ? (filesize - WRITE_SZ) : 0;

    unsigned int seed = (unsigned int)time(NULL) ^ (unsigned int)getpid();
    printf("Writing to file %s of size %lld bytes\n", path, (long long)filesize);
    uint64_t t_write0 = now_ns();
    for (long iter = 0; iter < iterations; ++iter) {
        off_t off = 0;
        if (max_offset > 0) {
            off = (off_t)((unsigned long)rand_r(&seed) % (unsigned long)(max_offset + 1));
        } else {
            off = 0;
        }

        if (lseek(fd, off, SEEK_SET) == (off_t)-1) {
            fprintf(stderr, "lseek(off=%lld) failed: %s\n",
                    (long long)off, strerror(errno));
            close(fd);
            return 1;
        }

        ssize_t nb = write(fd, buf, WRITE_SZ);
        if (nb != WRITE_SZ) {
            if (nb < 0)
                fprintf(stderr, "write failed: %s\n", strerror(errno));
            else
                fprintf(stderr, "short write: %zd/%d bytes\n", nb, WRITE_SZ);
            close(fd);
            return 1;
        }
    }
    uint64_t t_write1 = now_ns();

    uint64_t t_close0 = now_ns();
    int rc = close(fd);
    uint64_t t_close1 = now_ns();
    if (rc != 0) {
        fprintf(stderr, "close failed: %s\n", strerror(errno));
        return 1;
    }

    uint64_t t_total1 = now_ns();

    double open_ms  = ns_to_ms(t_open1  - t_open0);
    double write_ms = ns_to_ms(t_write1 - t_write0);
    double close_ms = ns_to_ms(t_close1 - t_close0);
    double total_ms = ns_to_ms(t_total1 - t_total0);
    double data_mb  = (iterations * (double)WRITE_SZ) / (1024.0 * 1024.0);

    printf("File: %s\n", path);
    printf("Iterations: %ld, WriteSize: %d bytes, FileSizeBound: %lld bytes\n",
           iterations, WRITE_SZ, (long long)filesize);
    printf("open:  %.3f ms\n", open_ms);
    printf("write: %.3f ms   (%.2f MB written, %.2f MB/s during write loop)\n",
           write_ms, data_mb, write_ms > 0.0 ? (data_mb / (write_ms / 1000.0)) : 0.0);
    printf("close: %.3f ms\n", close_ms);
    printf("total: %.3f ms\n", total_ms);

    return 0;
}
