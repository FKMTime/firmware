#include <Arduino.h>
#include <EEPROM.h>
#include <tuple>
#include <stackmat.h>
#include "ws_logger.h"

#if defined(ESP32)
 #include <ESPmDNS.h>
#elif defined(ESP8266)
 #include <ESP8266mDNS.h>
#endif

struct GlobalState {
  // TIMER INTERNALS
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  unsigned long solverCardId;
  unsigned long judgeCardId;
  String solverName;

  // STACKMAT
  StackmatTimerState lastTiemrState;
  bool stackmatConnected;

  // RFID
  unsigned long lastCardReadTime;
};

struct SavedState {
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  unsigned long solverCardId;
  unsigned long judgeCardId;
};

void stateDefault(GlobalState *state) {
  state->solveSessionId = 0;
  state->finishedSolveTime = -1;
  state->timeOffset = 0;
  state->solverCardId = 0;
  state->judgeCardId = 0;
}

void saveState(GlobalState state) {
  SavedState s;
  s.solveSessionId = state.solveSessionId;
  s.finishedSolveTime = state.finishedSolveTime;
  s.timeOffset = state.timeOffset;
  s.solverCardId = state.solverCardId;
  s.judgeCardId = state.judgeCardId;

  EEPROM.write(0, (uint8_t)sizeof(SavedState));
  EEPROM.put(1, s);
  EEPROM.commit();
}

void readState(GlobalState *state) {
  uint8_t size = EEPROM.read(0);
  Logger.printf("read Size: %d\n", size);
  if (size != sizeof(SavedState)) {
    Logger.println("Loading default state...");
    stateDefault(state);
    return;
  }

  SavedState _state;
  EEPROM.get(1, _state);

  state->solveSessionId = _state.solveSessionId;
  state->finishedSolveTime = _state.finishedSolveTime;
  state->timeOffset = _state.timeOffset;
  state->solverCardId = _state.solverCardId;
  state->judgeCardId = _state.judgeCardId;
}

String getChipID() {
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

  return "ws://0.0.0.0:0/";
}
