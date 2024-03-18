#ifndef __STATE_HPP__
#define __STATE_HPP__

#include <stackmat.h>
#include <UUID.h>
#include "ws_logger.h"

#define UUID_LENGTH 37

UUID uuid;
bool stateHasChanged = true;

enum StateScene {
  SCENE_NOT_INITALIZED, // before timer connects to wifi/ws
  SCENE_WAITING_FOR_COMPETITOR, // before competitor scans card
  SCENE_COMPETITOR_INFO, // competitor info with inspection info

  // FROM HERE, DO NOT SHOW TIMER/SERVER DISCONNECTED
  SCENE_INSPECTION, // during inspection (show inspection time etc)
  SCENE_TIMER_TIME, // during solve
  SCENE_FINISHED_TIME, // after solve
  SCENE_ERROR // after error
};

struct State {
    StateScene currentScene = SCENE_NOT_INITALIZED;

    char solveSessionId[UUID_LENGTH];
    int solveTime = 0;
    int penalty = 0;

    unsigned long competitorCardId = 0;
    unsigned long judgeCardId = 0;
    char competitorDisplay[128]; // max 128 chars

    bool timeConfirmed = false;
    bool waitingForSolveResponse = false;

    StackmatTimerState lastTimerState = ST_Unknown;
    bool stackmatConnected = false;
} state;

// TODO: save state to EEPROM
struct EEPROMState {
    char solveSessionId[UUID_LENGTH];
    unsigned long competitorCardId;
    int solveTime;
    int penalty;
};

void initState() {
  struct tm timeinfo;
  if (!getLocalTime(&timeinfo)) {
    Logger.println("Failed to obtain time");
  }
  time_t epoch;
  time(&epoch);

  uuid.seed(epoch, (unsigned long)ESP.getEfuseMac());

  state.currentScene = SCENE_WAITING_FOR_COMPETITOR;
}

/// @brief Called after time is finished
/// @param solveTime 
void startSolveSession(int solveTime) {
    uuid.generate(); // generate next uuid

    strncpy(state.solveSessionId, uuid.toCharArray(), UUID_LENGTH);
    state.solveTime = solveTime;
    state.penalty = 0;
    state.judgeCardId = 0;
    state.timeConfirmed = false;

    state.currentScene = SCENE_FINISHED_TIME;
}

void lcdStateManagementLoop() {
    if(!stateHasChanged) return;

    if (state.currentScene == SCENE_WAITING_FOR_COMPETITOR) {
        lcdPrintf(0, true, ALIGN_LEFT, "test");

        if (state.penalty == -1) {
            lcdPrintf(0, false, ALIGN_RIGHT, "DNF");
        } else if(state.penalty > 0) {
            lcdPrintf(0, false, ALIGN_RIGHT, "+%d", state.penalty);
        }
    }

    stateHasChanged = false;
}

#endif