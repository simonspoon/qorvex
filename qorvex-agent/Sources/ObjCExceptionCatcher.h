#ifndef ObjCExceptionCatcher_h
#define ObjCExceptionCatcher_h

#import <Foundation/Foundation.h>

/// Execute a block, catching any Objective-C NSException.
/// Returns YES on success, NO if an exception was caught (populating *error).
BOOL QVXTryCatch(void (NS_NOESCAPE ^_Nonnull block)(void),
                 NSError *_Nullable *_Nullable error);

#endif
