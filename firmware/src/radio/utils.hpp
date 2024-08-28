#ifndef __RADIO_UTILS__
#define __RADIO_UTILS__

#include <Arduino.h>

struct WsInfo {
  char host[100];
  char path[100];
  int port;
  bool ssl;
} typedef ws_info_t;

ws_info_t parseWsUrl(const char *url);
String getWsUrl();

#endif
