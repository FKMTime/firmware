#ifndef __GLOBALS_HPP__
#define __GLOBALS_HPP__

#define SLEEP_TIME 1800000 //30m

#include <LiquidCrystal_I2C.h>
#include <WebSocketsClient.h>
#include <Arduino.h>
#include <EEPROM.h>
#include "stackmat.h"

// Global websockets variable
WebSocketsClient webSocket;

// Global lcd variable
LiquidCrystal_I2C lcd(0x27, 16, 2);

// Global stackmat variable
Stackmat stackmat;

// Global state variable
struct GlobalState {
  // TIMER INTERNALS
  int solveSessionId;
  int lastFinishedSolveTime;
  int finishedSolveTime;
  int timeOffset;
  unsigned long competitorCardId;
  unsigned long judgeCardId;
  String competitorDisplay;

  bool timeStarted;
  bool timeConfirmed;
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
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  unsigned long competitorCardId;
};

void stateDefault() {
  state.solveSessionId = 0;
  state.finishedSolveTime = -1;
  state.timeOffset = 0;
  state.competitorCardId = 0;
}

void saveState() {
  SavedState s;
  s.solveSessionId = state.solveSessionId;
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

  SavedState _state;
  EEPROM.get(1, _state);

  state.solveSessionId = _state.solveSessionId;
  state.finishedSolveTime = _state.finishedSolveTime;
  state.timeOffset = _state.timeOffset;
  state.competitorCardId = _state.competitorCardId;
}

/// @brief Simple debug tool, for checking the state
void logState() {
  Logger.println("Current state:");
  Logger.printf("Solve sess id: %lu\n", state.solveSessionId);
  Logger.printf("Last finished time: %lu\n", state.lastFinishedSolveTime);
  Logger.printf("Finished time: %lu\n", state.finishedSolveTime);
  Logger.printf("Time offset: %lu\n", state.timeOffset);
  Logger.printf("Competitor card: %lu\n", state.competitorCardId);
  Logger.printf("Judge card: %lu\n", state.judgeCardId);
  Logger.printf("Competitor display: \"%s\"\n", state.competitorDisplay);
  Logger.printf("Time started: %lu\n", state.timeStarted);
  Logger.printf("Time confirmed: %lu\n", state.timeConfirmed);
  Logger.printf("Last time sent: %lu\n", state.lastTimeSent);
  Logger.printf("Errored: %lu\n", state.errored);
  Logger.printf("Last timer state: %lu\n", state.lastTimerState);
  Logger.printf("Stackmat connected: %lu\n", state.stackmatConnected);
  Logger.printf("Last card read: %lu\n\n", state.lastCardReadTime);
}

#endif