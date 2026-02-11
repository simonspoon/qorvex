#import "ObjCExceptionCatcher.h"

BOOL QVXTryCatch(void (NS_NOESCAPE ^_Nonnull block)(void),
                 NSError *_Nullable *_Nullable error) {
    @try {
        block();
        return YES;
    }
    @catch (NSException *exception) {
        if (error) {
            NSDictionary *userInfo = @{
                NSLocalizedDescriptionKey: exception.reason ?: exception.name,
                @"ExceptionName": exception.name
            };
            *error = [NSError errorWithDomain:@"com.qorvex.agent.objc-exception"
                                         code:1
                                     userInfo:userInfo];
        }
        return NO;
    }
}
