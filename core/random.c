#include <stdlib.h>
#include <time.h>
#include "random.h"

// randInt returns a random integer between 0 and the given parameter
int randInt(int upper) {
  srand(time(0));
  return rand() % upper;
}

// randString returns a random string of the given length
char* randString(int length) {
  char* result = (char*) malloc(length);

  srand(time(NULL));
  for( int i = 0; i < length; ++i){
    result[i] = '0' + rand() % 72;
  }

  return result;
}