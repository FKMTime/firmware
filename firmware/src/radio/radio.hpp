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

void initWifi() {
  WiFiManager wm;

  char generatedDeviceName[100];
  snprintf(generatedDeviceName, 100, "%s-%x", BLUETOOTH_PREFIX, getEspId());
  initBt(generatedDeviceName);

  char generatedSSID[100];
  snprintf(generatedSSID, 100, "%s-%x", WIFI_SSID_PREFIX, getEspId());
  wm.setConfigPortalTimeout(300);
  bool res = wm.autoConnect(generatedSSID, WIFI_PASSWORD);
  if (!res) {
    Logger.println("Failed to connect to wifi... Restarting!");
    delay(1500);
    ESP.restart();
  }

  deinitBt();
  configTime(3600, 0, "pool.ntp.org", "time.nist.gov", "time.google.com");
  initWs();
}

#endif