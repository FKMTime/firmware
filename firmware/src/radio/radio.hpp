#ifndef __RADIO_HPP__
#define __RADIO_HPP__

#include <WiFi.h>
#include <WiFiManager.h>
#include <ws_logger.h>
#include "bt.hpp"
#include "lcd.hpp"
#include "version.h"
#include "websocket.hpp"
#include "defines.h"

void apCallback(WiFiManager *wm);

void initWifi() {
  WiFi.mode(WIFI_STA); 
  WiFiManager wm;

  char generatedDeviceName[100];
  snprintf(generatedDeviceName, 100, "%s-%x", NAME_PREFIX, getEspId());

  wm.setConfigPortalTimeout(300);
  wm.setConfigPortalBlocking(false);
  wm.setConnectRetries(10);
  wm.setConnectTimeout(3);

  bool res = true;
  for(int i = 0; i < 5; i++) {
    res = wm.autoConnect(generatedDeviceName, WIFI_PASSWORD);
    if (res) break;
  }
  
  if (res) {
    Logger.printf("Connected!\n");
  } else {
    initBt(generatedDeviceName);
  }

  while(!res && !wm.process()) {
    delay(5);
  }

  wifiConnected = true;
  if (!res) deinitBt(true);
  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
  initWs();
}

#endif