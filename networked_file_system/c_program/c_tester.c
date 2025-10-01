#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdio.h>
#include <assert.h>

int
main() {
   int fd0 = open("/mnt/netfs/foo", O_RDWR);
   if (fd0 < 0) {
        fprintf(stderr, "Error opening file 'foo'\n");
        perror("open");
        return 1;
   }
   int fd1 = open("/mnt/netfs/foo", O_RDONLY);
   if (fd1 < 0) {
       fprintf(stderr, "Error opening file 'foo'\n");
       perror("open");
       return 1;
   }
   char buf[100];
   buf[0] = 9;
   buf[1] = 81;
   buf[2] = 'A';
   buf[3] = 'q';
   buf[4] = '0';
   int nb0 = write(fd0, buf, 100);
   if (nb0 < 0) {
       fprintf(stderr, "Error writing file 'foo'\n");
       perror("write");
       return 1;
   }
   int nb1;
   close(fd0);
   nb1 = read(fd1, buf, 100);
   if (nb1 < 0) {
       fprintf(stderr, "Error reading file 'foo'\n");
       perror("read");
       return 1;
   }
   assert(buf[0] == 9);
   assert(buf[1] == 81);
   assert(buf[2] == 'A');
   assert(buf[3] == 'q');
   assert(buf[4] == '0');
   close(fd1);
   printf("Wrote %d, then %d bytes\n", nb0, nb1);
   return 0;
}