#ifndef __UTILS_HPP_
#define __UTILS_HPP_

#include "globals.hpp"
#include "ws_logger.h"
#include <Arduino.h>
#include <stackmat.h>
#include <tuple>

#if defined(ESP32)
#include <ESPmDNS.h>
#elif defined(ESP8266)
#include <ESP8266mDNS.h>
#endif

String getChipHex() {
  uint64_t chipid = ESP_ID();
  String chipidStr =
      String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
  return chipidStr;
}

std::tuple<std::string, int, std::string> parseWsUrl(std::string url) {
  int port;
  std::string path;

  if (url.rfind("ws://", 0) == 0) {
    url = url.substr(5);
    port = 80;
  } else if (url.rfind("wss://", 0) == 0) {
    url = url.substr(6);
    port = 443;
  } else {
    return {"", -1, ""};
  }

  int pathSplitPos = url.find("/");
  if ((std::size_t)pathSplitPos == std::string::npos) {
    pathSplitPos = url.length();
    url = url + "/";
  }

  path = url.substr(pathSplitPos);
  url = url.substr(0, pathSplitPos);

  int portSplitPos = url.rfind(":");
  if ((std::size_t)portSplitPos != std::string::npos) {
    port = stoi(url.substr(portSplitPos + 1));
    url = url.substr(0, portSplitPos);
  }

  return {url, port, path};
}

String getWsUrl() {
  if (!MDNS.begin("random")) {
    Logger.printf("Failed to setup MDNS!");
  }

  int n = MDNS.queryService("stackmat", "tcp");
  if (n > 0) {
    Logger.printf("Found stackmat MDNS:\n Hostname: %s, IP: %s, PORT: %d\n",
                  MDNS.hostname(0).c_str(), MDNS.IP(0).toString().c_str(),
                  MDNS.port(0));
    return MDNS.hostname(0);
  }
  MDNS.end();

  return "";
}

void sendSolve(bool delegate) {
  struct tm timeinfo;
  if (!getLocalTime(&timeinfo)) {
    Logger.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);

  JsonDocument doc;
  doc["solve"]["solve_time"] = state.finishedSolveTime;
  doc["solve"]["offset"] = state.timeOffset;
  doc["solve"]["competitor_id"] = state.competitorCardId;
  doc["solve"]["judge_id"] = state.judgeCardId;
  doc["solve"]["esp_id"] = ESP_ID();
  doc["solve"]["timestamp"] = epoch;
  doc["solve"]["session_id"] = state.solveSessionId;
  doc["solve"]["delegate"] = delegate;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);

  // if delegate is called before time has started, timer shouldnt wait for response
  state.waitingForSolveResponse = state.timeStarted;
}

String displayTime(uint8_t m, uint8_t s, uint16_t ms) {
  String tmp = "";
  if (m > 0) {
    tmp += m;
    tmp += ":";

    char sBuff[6];
    sprintf(sBuff, "%02d", s);
    tmp += String(sBuff);
  } else {
    tmp += s;
  }

  char msBuff[6];
  sprintf(msBuff, "%03d", ms);

  tmp += ".";
  tmp += String(msBuff);
  return tmp;
}

float analogReadAvg(int pin, int c = 10) {
  float v = 0;
  for (int i = 0; i < c; i++) {
    v += analogRead(pin);
  }

  return v / c;
}

float voltageToPercentage(float voltage) {
  const float minVoltage = 3.0;
  const float maxVoltage = 4.1;
  float percentage = 0.0;

  // Ensure the voltage is within the expected range
  if (voltage < minVoltage) {
    voltage = minVoltage;
  } else if (voltage > maxVoltage) {
    voltage = maxVoltage;
  }

  // Calculate the percentage
  percentage = ((voltage - minVoltage) / (maxVoltage - minVoltage)) * 100;

  return percentage;
}

#define V_REF 3.3
#define READ_OFFSET 1.08
#define MAX_ADC 4095.0 // max adc - 1
#define R1 10000
#define R2 10000
float readBatteryVoltage(int pin) {
  float val = analogReadAvg(pin);
  float voltage = val * READ_OFFSET * (V_REF / MAX_ADC) * ((R1 + R2) / R2);

  return voltage + 0.1;
}

#endif
