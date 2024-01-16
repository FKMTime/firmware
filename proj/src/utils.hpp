#include <Arduino.h>
#include <EEPROM.h>
#include <tuple>
#include <stackmat.h>
#include "ws_logger.h"

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

  Logger.println("State to save:");
  Logger.printf("SessId: %d\n", s.solveSessionId);
  Logger.printf("Last Time: %d\n",s.finishedSolveTime);
  Logger.printf("Time offset: %d\n", s.timeOffset);
  Logger.printf("Solver CID: %lu\n", s.solverCardId);
  Logger.printf("Judge CID: %lu\n", s.judgeCardId);

  byte* buff = (byte*)malloc(sizeof(SavedState));
  memcpy(&buff, &s, sizeof(SavedState));
  
  EEPROM.write(0, sizeof(SavedState));
  int offset = 1;

  for(unsigned int i = 0; i < sizeof(SavedState); i++) {
    EEPROM.write(i + offset, buff[i]);
  }

  EEPROM.commit();
}

void readState(GlobalState *state) {
  unsigned int size = EEPROM.read(0);
  if (size != sizeof(SavedState)) {
    Logger.println("Loading default state...");
    stateDefault(state);
    return;
  }

  int offset = 1;
  byte* buff = (byte*)malloc(sizeof(SavedState));
  for (unsigned int i = 0; i < sizeof(SavedState); i++) {
    buff[i] = EEPROM.read(i + offset);
  }

  SavedState _state;
  memcpy(&_state, &buff, sizeof(SavedState));

  state->solveSessionId = _state.solveSessionId;
  state->finishedSolveTime = _state.finishedSolveTime;
  state->timeOffset = _state.timeOffset;
  state->solverCardId = _state.solverCardId;
  state->judgeCardId = _state.judgeCardId;

  Logger.println("Loaded state:");
  Logger.printf("SessId: %d\n", _state.solveSessionId);
  Logger.printf("Last Time: %d\n", _state.finishedSolveTime);
  Logger.printf("Time offset: %d\n", _state.timeOffset);
  Logger.printf("Solver CID: %lu\n", _state.solverCardId);
  Logger.printf("Judge CID: %lu\n", _state.judgeCardId);
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