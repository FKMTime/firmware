#ifndef __RADIO_UTILS__
#define __RADIO_UTILS__

#include <Arduino.h>
#include <ESPmDNS.h>

struct WsInfo {
    char host[100];
    int port;
    char path[100];
} typedef ws_info_t;

ws_info_t parseWsUrl(const char *url) {
  ws_info_t wsInfo = {0};
  int pathPtr = 0;

  if (strncmp("ws://", url, 5) == 0) {
    pathPtr = 5;
    wsInfo.port = 80;
  } else if (strncmp("wss://", url, 6) == 0) {
    pathPtr = 6;
    wsInfo.port = 443;
  } else {
    return wsInfo;
  }

  // url with offset of pathPtr
  char *pathSplitPtr = strchr(url + pathPtr, '/');
  int pathSplitIdx = pathSplitPtr == NULL ? strlen(url) : pathSplitPtr - url;

  if (pathSplitPtr != NULL) {
    strncpy(wsInfo.path, pathSplitPtr, 100);
  }
  strncpy(wsInfo.host, url + pathPtr, pathSplitIdx - pathPtr);

  // snprintf(wsInfo.host, pathSplitIdx - pathPtr + 1, url + pathPtr);
  if (strlen(wsInfo.path) == 0) {
    wsInfo.path[0] = '/';
    wsInfo.path[1] = '\0';
  }

  char *portSplitPtr = strchr(wsInfo.host, ':');
  if (portSplitPtr != NULL) {
    char portStr[10];

    int idx = portSplitPtr - wsInfo.host;
    strcpy(portStr, wsInfo.host + idx + 1);

    wsInfo.host[idx] = '\0';
    wsInfo.port = atoi(portStr);
  }

  return wsInfo;
}

String getWsUrl() {
  if (!MDNS.begin("random")) {
    Logger.printf("Failed to setup MDNS!");
  }

  int n = MDNS.queryService("stackmat", "tcp");
  if (n > 0) {
    Logger.printf("Found stackmat MDNS:\nHostname: %s, IP: %s, PORT: %d\n",
                  MDNS.txt(0, "ws").c_str(), MDNS.IP(0).toString().c_str(),
                  MDNS.port(0));
    return MDNS.txt(0, "ws");
  }
  MDNS.end();

  return "";
}

#endif