#ifndef __GLOBALS_HPP__
#define __GLOBALS_HPP__

#define SLEEP_TIME 1800000 //30m

#include <LiquidCrystal_I2C.h>
#include <WebSocketsClient.h>
#include <Arduino.h>
#include <EEPROM.h>
#include "ws_logger.h"
#include "stackmat.h"
#include "UUID.h"

char *generateUUID();

// Global websockets variable
WebSocketsClient webSocket;

// Global lcd variable
LiquidCrystal_I2C lcd(0x27, 16, 2);

// Global stackmat variable
Stackmat stackmat;

UUID uuid;

// Global state variable
struct GlobalState {
  // TIMER INTERNALS
  char solveSessionId[37];
  int lastFinishedSolveTime = -1;
  int finishedSolveTime = -1;
  int timeOffset = 0;
  unsigned long competitorCardId;
  unsigned long judgeCardId;
  String competitorDisplay;

  bool timeStarted = false;
  bool timeConfirmed = false;
  bool waitingForSolveResponse = false;
  unsigned long lastTimeSent;

  bool errored;

  // STACKMAT
  StackmatTimerState lastTimerState;
  bool stackmatConnected;

  // RFID
  unsigned long lastCardReadTime;
} state;

bool sleepMode = false;
bool primaryLangauge = true;

struct SavedState {
  unsigned long competitorCardId;
  int finishedSolveTime;
  int timeOffset;
  char solveSessionId[37];
};

void stateDefault() {
  strcpy(state.solveSessionId, generateUUID());
  state.finishedSolveTime = -1;
  state.timeOffset = 0;
  state.competitorCardId = 0;
}

void saveState() {
  SavedState s = {0};
  strcpy(s.solveSessionId, state.solveSessionId);
  s.finishedSolveTime = state.finishedSolveTime;
  s.timeOffset = state.timeOffset;
  s.competitorCardId = state.competitorCardId;

  EEPROM.write(0, (uint8_t)sizeof(SavedState));
  EEPROM.put(1, s);
  EEPROM.commit();
}

void readState() {
  uint8_t size = EEPROM.read(0);
  Logger.printf("read Size: %d\n", size);
  if (size != sizeof(SavedState)) {
    Logger.println("Loading default state...");
    stateDefault();
    return;
  }

  SavedState _state = {0};
  EEPROM.get(1, _state);

  strcpy(state.solveSessionId, _state.solveSessionId);
  state.finishedSolveTime = _state.finishedSolveTime;
  state.timeOffset = _state.timeOffset;
  state.competitorCardId = _state.competitorCardId;
  state.timeStarted = state.finishedSolveTime > 0;
}

/// @brief Simple debug tool, for checking the state
void logState() {
  Logger.println("Current state:");
  Logger.printf("Solve sess id: %s\n", state.solveSessionId);
  Logger.printf("Last finished time: %d\n", state.lastFinishedSolveTime);
  Logger.printf("Finished time: %d\n", state.finishedSolveTime);
  Logger.printf("Time offset: %d\n", state.timeOffset);
  Logger.printf("Competitor card: %lu\n", state.competitorCardId);
  Logger.printf("Judge card: %lu\n", state.judgeCardId);
  Logger.printf("Competitor display: \"%s\"\n", state.competitorDisplay);
  Logger.printf("Time started: %d\n", state.timeStarted);
  Logger.printf("Time confirmed: %d\n", state.timeConfirmed);
  Logger.printf("Last time sent: %lu\n", state.lastTimeSent);
  Logger.printf("Errored: %d\n", state.errored);
  Logger.printf("Last timer state: %lu\n", state.lastTimerState);
  Logger.printf("Stackmat connected: %d\n", state.stackmatConnected);
  Logger.printf("Last card read: %lu\n\n", state.lastCardReadTime);
}

void initUUID() {
  struct tm timeinfo;
  if (!getLocalTime(&timeinfo))
  {
    Logger.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);

  uuid.seed(epoch, ESP_ID());
}

char *generateUUID() {
  uuid.generate();

  return uuid.toCharArray();
}

#endif