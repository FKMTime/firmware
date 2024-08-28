#ifndef __STATE_HPP__
#define __STATE_HPP__

#include <UUID.h>
#include <stackmat.h>

#define UUID_LENGTH 37

enum StateScene {
  SCENE_NOT_INITALIZED,                   // before timer connects to wifi/ws
  SCENE_WAITING_FOR_COMPETITOR,           // before competitor scans card
  SCENE_WAITING_FOR_COMPETITOR_WITH_TIME, // after competitor solved but didn't
                                          // scan his card
  SCENE_COMPETITOR_INFO, // competitor info with inspection info

  // FROM HERE, DO NOT SHOW TIMER/SERVER DISCONNECTED
  SCENE_INSPECTION,    // during inspection (show inspection time etc)
  SCENE_TIMER_TIME,    // during solve
  SCENE_FINISHED_TIME, // after solve
  SCENE_ERROR          // after error
};

struct State {
  StateScene currentScene = SCENE_NOT_INITALIZED;

  char solveSessionId[UUID_LENGTH];
  int solveTime = 0;
  int lastSolveTime = 0;
  int penalty = 0;

  bool added = true;
  bool useInspection = true;
  char secondaryText[32];
  unsigned long inspectionStarted = 0;
  unsigned long inspectionEnded = 0;

  unsigned long competitorCardId = 0;
  unsigned long judgeCardId = 0;
  char competitorDisplay[128]; // max 128 chars

  bool timeConfirmed = false;
  bool testMode = false;

  StackmatTimerState lastTimerState = ST_Unknown;

  char errorMsg[128]; // max 128 chars
  StateScene sceneBeforeError = SCENE_NOT_INITALIZED;
};

struct EEPROMState {
  char solveSessionId[UUID_LENGTH];
  unsigned long competitorCardId;
  unsigned long inspectionStarted;
  unsigned long inspectionEnded;
  unsigned long saveTime;
  int solveTime;
  int penalty;

  float batteryOffset;
};

extern State state;
extern EEPROMState eepromState;

extern UUID uuid;
extern bool stateHasChanged;
extern bool lockStateChange;
extern bool waitForSolveResponse;
extern bool waitForDelegateResponse;

extern int testModeStackmatTime; // mock of stackmat time for testmode

void stateDefault();
void saveState();
void readState();
void initState();
void checkConnectionStatus();
void stateLoop();
void startSolveSession(int solveTime);
void resetSolveState(bool save = true);
void startInspection();
void stopInspection();
void showError(const char *str);
void sendSolve(bool delegate);
void scanCard(unsigned long cardId);
void sendSnapshotData();
void sendTestAck();
void logState();

#endif
