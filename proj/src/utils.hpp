#ifndef __UTILS_HPP_
#define __UTILS_HPP_

#include <Arduino.h>
#include <tuple>
#include <stackmat.h>
#include "ws_logger.h"
#include "globals.hpp"

#if defined(ESP32)
 #include <ESPmDNS.h>
#elif defined(ESP8266)
 #include <ESP8266mDNS.h>
#endif

String getChipHex() {
  uint64_t chipid = ESP_ID();
  String chipidStr = String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
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

String centerString(String str, int size) {
  int padSize = size - str.length();
  if (padSize <= 0) return str;

  int padLeft = padSize / 2;
  int padRight = padSize - padLeft;

  String tmp;
  for (int i = 0; i < padLeft; i++) tmp += " ";
  tmp += str;
  for (int i = 0; i < padRight; i++) tmp += " ";

  return tmp;
}

String getWsUrl() {
  if (!MDNS.begin("random")) {
    Logger.printf("Failed to setup MDNS!");
  }

  int n = MDNS.queryService("stackmat", "tcp");
  if (n > 0) {
    Logger.printf("Found stackmat MDNS:\n Hostname: %s, IP: %s, PORT: %d\n", MDNS.hostname(0).c_str(), MDNS.IP(0).toString().c_str(), MDNS.port(0));
    return MDNS.hostname(0);
  }
  MDNS.end();

  return "";
}

void sendSolve(bool delegate) {
  if (state.finishedSolveTime == -1) return;

  struct tm timeinfo;
  if (!getLocalTime(&timeinfo))
  {
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

#endif