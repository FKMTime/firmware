#ifndef __GLOBALS_HPP__
#define __GLOBALS_HPP__

#include <WebSocketsClient.h>
#include <Arduino.h>
#include <EEPROM.h>
#include "stackmat.h"
#include "rgb_lcd.h"

// Global websockets variable
WebSocketsClient webSocket;

// Global lcd variable
rgb_lcd lcd;

// Global stackmat variable
Stackmat stackmat;

// Global state variable
struct GlobalState {
  // TIMER INTERNALS
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  unsigned long solverCardId;
  unsigned long judgeCardId;
  String solverName;

  bool timeStarted;
  bool timeConfirmed;
  unsigned long lastTimeSent;

  // STACKMAT
  StackmatTimerState lastTiemrState;
  bool stackmatConnected;

  // RFID
  unsigned long lastCardReadTime;
} state;

struct SavedState {
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  unsigned long solverCardId;
};

void stateDefault(GlobalState *state) {
  state->solveSessionId = 0;
  state->finishedSolveTime = -1;
  state->timeOffset = 0;
  state->solverCardId = 0;
}

void saveState(GlobalState state) {
  SavedState s;
  s.solveSessionId = state.solveSessionId;
  s.finishedSolveTime = state.finishedSolveTime;
  s.timeOffset = state.timeOffset;
  s.solverCardId = state.solverCardId;

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
}

#endif